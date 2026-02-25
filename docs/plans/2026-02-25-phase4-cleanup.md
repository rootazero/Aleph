# Phase 4: Cleanup — Deprecate Swift App, Remove Bindings, Update Docs

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Complete the Server-Centric Build Architecture migration by deprecating the macOS Swift app, removing UniFFI bindings, and updating all documentation to reflect the new Daemon + Bridge architecture.

**Architecture:** With Phase 3 done, all desktop capabilities live in the Tauri bridge. The Swift app (`apps/macos/`) and UniFFI bindings (`core/bindings/`) are no longer needed. This phase cleans them up and updates the project's documentation to match reality.

**Tech Stack:** Git, Markdown, Cargo.toml workspace config

---

### Task 1: Deprecate `apps/macos/` Swift App

**Files:**
- Create: `apps/macos/DEPRECATED.md`
- Modify: `apps/macos/project.yml` (if XcodeGen is still configured)

**Context:** The Swift macOS app was the original UI shell. Its functionality has been fully replaced by `aleph-server` (brain) + `aleph-bridge` (Tauri body). We mark it as deprecated rather than deleting, preserving it as reference material for the Swift implementations of Perception, Action, and Canvas.

**Step 1: Create deprecation notice**

Create `apps/macos/DEPRECATED.md`:

```markdown
# DEPRECATED — macOS Swift App

> **Status:** Deprecated as of 2026-02-25
> **Replaced by:** `aleph-server` (Rust, single binary) + `aleph-bridge` (Tauri, cross-platform)

## Why Deprecated

The Server-Centric Build Architecture (see `docs/plans/2026-02-25-server-centric-build-architecture-design.md`) replaces this Swift app with:

1. **aleph-server** — The AI brain. Single Rust binary with Gateway, Agent Loop, Panel UI (Leptos/WASM), and Bridge Manager.
2. **aleph-bridge** (Tauri) — The system shell. Provides desktop capabilities (screenshot, OCR, keyboard, mouse, AX inspection, canvas overlay, tray, hotkey) via UDS JSON-RPC 2.0.

## What Replaced What

| Swift Component | New Location | Technology |
|-----------------|-------------|------------|
| ScreenCaptureKit (screenshot) | `apps/desktop/src-tauri/src/bridge/perception.rs` | xcap (cross-platform) |
| Vision (OCR) | `apps/desktop/src-tauri/src/bridge/perception.rs` | objc + Vision (macOS) |
| Accessibility (AX tree) | `apps/desktop/src-tauri/src/bridge/perception.rs` | objc + AX API (macOS) |
| CoreGraphics (mouse/keyboard) | `apps/desktop/src-tauri/src/bridge/action.rs` | enigo (cross-platform) |
| NSWorkspace (app launch) | `apps/desktop/src-tauri/src/bridge/action.rs` | std::process::Command |
| WKWebView (canvas overlay) | `apps/desktop/src-tauri/src/bridge/canvas.rs` | Tauri WebView |
| NSStatusItem (tray) | `apps/desktop/src-tauri/src/tray.rs` | Tauri tray plugin |
| HaloWindow (floating UI) | Tauri window config | Tauri window API |
| All SwiftUI business pages | `core/ui/control_plane/` | Leptos/WASM |
| GatewayClient (WebSocket) | N/A | UDS replaces WebSocket for bridge |

## Preserved For Reference

The Swift implementations in `Sources/DesktopBridge/` contain well-documented examples of:
- Vision OCR with multi-language support
- Accessibility tree walking with AXUIElement
- CoreGraphics mouse/keyboard event generation
- ScreenCaptureKit capture with region support
- Canvas overlay with A2UI patch protocol

These serve as reference implementations for the Rust/Tauri equivalents.

## Build Instructions (Legacy)

These are preserved for historical reference only:
```
cd apps/macos && xcodegen generate && open Aleph.xcodeproj
```

## Do Not Delete

Keep this directory until the Tauri bridge has been battle-tested in production for at least one release cycle.
```

**Step 2: Verify**

Confirm the file was created:
```bash
ls apps/macos/DEPRECATED.md
```

**Step 3: Commit**

```bash
git add apps/macos/DEPRECATED.md
git commit -m "macos: mark Swift app as deprecated in favor of Tauri bridge"
```

---

### Task 2: Remove `core/bindings/` (if present)

**Files:**
- Remove: `core/bindings/` directory (UniFFI bindings)

**Context:** UniFFI bindings were used to call Rust core from Swift via C FFI (`libalephcore.dylib`). With the Daemon + Bridge architecture, the server and bridge are separate processes communicating via UDS — no FFI needed.

**Step 1: Check if bindings exist**

```bash
ls -la core/bindings/ 2>/dev/null
```

If the directory exists:
```bash
git rm -r core/bindings/
```

If it doesn't exist, skip to Step 3.

**Step 2: Remove any Cargo.toml references**

Check if `core/Cargo.toml` has any `uniffi`-related dependencies or build scripts referencing bindings:

```bash
grep -r "uniffi\|bindings" core/Cargo.toml
```

Remove any UniFFI-related entries.

**Step 3: Verify build**

```bash
cargo check --bin aleph-server --features control-plane
```

**Step 4: Commit** (only if changes were made)

```bash
git add -A core/bindings/ core/Cargo.toml
git commit -m "core: remove UniFFI bindings (replaced by UDS IPC)"
```

---

### Task 3: Update CLAUDE.md Architecture Documentation

**Files:**
- Modify: `/CLAUDE.md`

**Context:** CLAUDE.md still references the old architecture in several places. Update to reflect the Server-Centric Build Architecture.

**Step 1: Update project structure section**

Update the `apps/` tree to reflect current state:

```
├── apps/
│   ├── cli/                        # Rust CLI 客户端
│   ├── macos/                      # [DEPRECATED] macOS Swift App
│   └── desktop/                    # Cross-platform Tauri Bridge (aleph-bridge)
```

**Step 2: Update build commands**

Replace the existing build commands section with:

```bash
# Rust Core
cd core && cargo build && cargo test

# 启动 Server (不含 Control Plane UI)
cargo run --bin aleph-server

# 启动 Server (含 Control Plane UI)
cargo run --bin aleph-server --features control-plane

# Build Bridge (cross-platform)
cd apps/desktop && cargo tauri build

# [DEPRECATED] macOS App (保留仅供参考)
# cd apps/macos && xcodegen generate && open Aleph.xcodeproj
```

**Step 3: Update 3 Limbs description**

The Native 能力 section should mention the Tauri bridge:

```
| **Native 能力 (The Muscles)** | 直接控制系统 | Desktop Bridge (Tauri/Rust) — "看"(OCR/截图) 和 "动"(点击/输入)；Shell 执行 (Bash) |
```

**Step 4: Add Bridge Architecture note to 核心子系统 table**

Add a row:

```
| **Desktop Bridge** | UDS JSON-RPC 2.0 桥接，桌面能力 (OCR/截图/输入/窗口/Canvas) | [Design](docs/plans/2026-02-25-server-centric-build-architecture-design.md) |
```

**Step 5: Verify no broken references**

Ensure no documentation references `libalephcore.dylib` or assumes the Swift app is the primary UI.

**Step 6: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: update CLAUDE.md for Server-Centric Build Architecture"
```

---

### Task 4: Update Design Docs Index

**Files:**
- Modify: `CLAUDE.md` (设计文档 section)

**Step 1: Add Phase 1-4 plan references**

In the 设计文档 table, add:

```markdown
| [Server-Centric Build Architecture](docs/plans/2026-02-25-server-centric-build-architecture-design.md) | Daemon + Bridge 架构设计 |
| [Phase 1: Bridge Skeleton](docs/plans/2026-02-25-phase1-bridge-skeleton.md) | IPC + WebView + Tray + Hotkey |
| [Phase 2.5: Bridge Integration](docs/plans/2026-02-25-phase2.5-bridge-integration-completion.md) | Capability parsing, dynamic URLs, socket unification |
| [Phase 3: Desktop Capabilities](docs/plans/2026-02-25-phase3-desktop-capabilities.md) | OCR, input control, window management, canvas |
| [Phase 4: Cleanup](docs/plans/2026-02-25-phase4-cleanup.md) | Deprecation and documentation |
```

**Step 2: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: add Server-Centric Build plan references to CLAUDE.md"
```

---

### Task 5: Final Verification

**Step 1: Full build**

```bash
cargo check --bin aleph-server --features control-plane
cargo check -p aleph-tauri
```

**Step 2: Run all tests**

```bash
cargo test --lib -p alephcore -- desktop
cargo test --lib -p alephcore -- gateway::bridge
cargo test --lib -p alephcore -- builtin_tools::desktop
```

**Step 3: Verify no stale references**

```bash
grep -r "libalephcore" core/ apps/ --include="*.rs" --include="*.toml" | grep -v DEPRECATED
grep -r "UniFFI\|uniffi" core/ --include="*.rs" --include="*.toml"
```

Expected: No results (or only in deprecated/docs).

**Step 4: Summary commit (if any fixups needed)**

```bash
git commit -m "phase4: cleanup verified — Server-Centric Build Architecture complete"
```

---

## Summary of Changes

| File | Action | Purpose |
|------|--------|---------|
| `apps/macos/DEPRECATED.md` | Create | Deprecation notice with migration mapping |
| `core/bindings/` | Remove (if exists) | UniFFI no longer needed |
| `CLAUDE.md` | Modify | Update architecture docs, build commands, plan references |

## What's Preserved

- `apps/macos/` — Kept as reference (marked DEPRECATED)
- `apps/cli/` — Pure I/O reference client (still active)
- `apps/desktop/` — Promoted to primary bridge (actively developed)

## Architectural Redline Compliance Check

After Phase 4:
- **R1 (Brain-Limb Separation)**: Server contains zero platform APIs. All desktop ops via UDS to Bridge. ✅
- **R2 (Single UI Source)**: All business UI in Leptos/WASM Panel. Bridge only hosts WebView container + tray. ✅
- **R3 (Core Minimalism)**: No heavy platform deps in core. ✅
- **R4 (I/O-Only Interfaces)**: Bridge does zero business logic. Receives commands, executes system operations, returns results. ✅
