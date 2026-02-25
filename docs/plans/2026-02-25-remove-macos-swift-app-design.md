# Remove Deprecated macOS Swift App — Design Document

> **Status**: Approved
> **Date**: 2026-02-25
> **Scope**: Full cleanup of `apps/macos/`, `core/bindings/`, and all Swift/UniFFI references
> **Prerequisite**: Acceptance tests must pass before cleanup begins

---

## Background

The macOS Swift App (`apps/macos/`) was deprecated on 2026-02-25 as part of the Server-Centric Build Architecture transition. The Tauri Bridge (`apps/desktop/`) now provides full functional parity across macOS, Windows, and Linux.

This design document defines:
1. **Acceptance Matrix** — what must pass before cleanup
2. **Test Plan** — how to verify each criterion
3. **Cleanup Checklist** — what to delete and in what order
4. **Documentation Updates** — what to update after cleanup

---

## 1. Acceptance Matrix

### 1.1 Functional Parity (F1-F12)

| # | Swift Capability | Tauri Implementation | Pass Criteria |
|---|-----------------|---------------------|---------------|
| F1 | Screenshot (ScreenCaptureKit) | `perception.rs` — xcap | RPC returns valid base64 PNG with correct dimensions |
| F2 | OCR (Vision.framework) | `perception.rs` — objc + Vision | Chinese/English recognition accuracy >= Swift version |
| F3 | AX Tree (NSAccessibility) | `perception.rs` — objc + AX API | Returns element tree with depth >= 3 for standard apps (Finder, Safari) |
| F4 | Mouse click/drag/scroll (CoreGraphics) | `action.rs` — enigo | Click specified coordinates, drag, scroll on macOS |
| F5 | Keyboard input (CGEvent) | `action.rs` — enigo | Type Chinese/English text, Cmd+C/V combos work |
| F6 | App launch (NSWorkspace) | `action.rs` — std::process::Command | Launch any app by bundle ID |
| F7 | Window list/focus (CGWindowList) | `action.rs` — CGWindowListCopyWindowInfo | Return complete window list, focus specific windows |
| F8 | Canvas overlay (WKWebView) | `canvas.rs` — Tauri WebView | Show transparent overlay with HTML injection and A2UI patch |
| F9 | Tray icon (NSStatusItem) | `tray.rs` — Tauri tray plugin | Tray menu functional, Provider switching works |
| F10 | Halo floating window | Tauri Window (transparent, borderless) | Hotkey show/hide, centered bottom position |
| F11 | Settings window | Tauri Window + Leptos Control Plane | Open settings, read/write config persistence |
| F12 | Handshake + capability registration | `bridge/mod.rs` | Bridge auto-registers all capabilities, Server identifies them |

### 1.2 End-to-End Integration (E1-E6)

| # | Flow | Pass Criteria |
|---|------|---------------|
| E1 | Server startup -> auto-spawn Bridge subprocess | Bridge process starts and completes handshake within 5s |
| E2 | Bridge -> UDS connection -> capability registration | Server logs show received capability list |
| E3 | User hotkey -> Halo window -> input -> AI response | Full conversation flow without errors |
| E4 | Server shutdown -> Bridge graceful exit | No zombie processes, socket file cleaned up |
| E5 | Bridge crash -> Server detects and restarts | Server logs show reconnection, functionality restored |
| E6 | Server without Bridge -> headless mode | Gateway API and CLI work normally, desktop features return ERR_NOT_AVAILABLE |

### 1.3 Performance Benchmarks (P1-P5)

| # | Metric | Baseline (Swift) | Target (Tauri) | Measurement |
|---|--------|-----------------|----------------|-------------|
| P1 | Screenshot latency | Measure | <= Swift x1.2 | 100 consecutive captures, median |
| P2 | OCR latency | Measure | <= Swift x1.5 | Same image OCR 50 times, median |
| P3 | Input latency | Measure | <= 50ms perceived | type_text 1000 chars, timed |
| P4 | Bridge startup time | Measure | <= 3s (process start to handshake) | 10 cold starts, median |
| P5 | Memory usage | Measure | <= Swift x1.5 | Idle 30min, RSS |

### 1.4 Stability (S1-S4)

| # | Test | Pass Criteria |
|---|------|---------------|
| S1 | Continuous run 8h | No crashes, no memory leaks (RSS growth < 20%) |
| S2 | 100 hotkey show/hide cycles | All successful, no window artifacts |
| S3 | UDS disconnect/reconnect 10 times | Functionality normal after each reconnect |
| S4 | High-frequency RPC (10 req/s for 5min) | No timeouts, stable response times |

---

## 2. Test Plan

### 2.1 Infrastructure

**Test script location**: `tests/bridge_acceptance/`

**Dependencies**:
- `socat` — UDS communication (built-in on macOS)
- `jq` — JSON parsing
- `time` — performance timing
- Swift build artifacts (for baseline measurement only; can be deleted after)

### 2.2 Automated Test Scripts

#### A. Functional Parity (`test_functional_parity.sh`)

Each F1-F12 maps to an RPC call test:

```bash
# Common pattern: send JSON-RPC to UDS, validate response
send_rpc() {
  echo "$1" | socat - UNIX-CONNECT:~/.aleph/bridge.sock
}

# F1: Screenshot
result=$(send_rpc '{"jsonrpc":"2.0","id":"f1","method":"desktop.screenshot","params":{}}')
assert_json_has "$result" ".result.image"     # base64 string
assert_json_has "$result" ".result.width"      # > 0
assert_json_has "$result" ".result.height"     # > 0
```

Covers all 12 functional items, outputs PASS/FAIL summary table.

#### B. End-to-End Flow (`test_e2e_flow.sh`)

```bash
# E1: Server spawns Bridge
start_server_with_bridge()
wait_for_log "Desktop bridge started" 5  # 5s timeout

# E4: Graceful shutdown
send_shutdown_signal()
wait_for_process_exit $BRIDGE_PID 3      # 3s timeout
assert_file_missing ~/.aleph/bridge.sock

# E5: Bridge crash recovery
kill -9 $BRIDGE_PID
wait_for_log "Bridge reconnected" 10     # 10s timeout
```

#### C. Performance Benchmark (`test_performance.sh`)

```bash
# P1: Screenshot latency
benchmark "desktop.screenshot" 100  # 100 times, output min/median/p95/max

# P2: OCR latency
benchmark "desktop.ocr" 50

# P4: Bridge startup time
for i in $(seq 1 10); do
  cold_start_and_measure_handshake_time
done
```

For Swift baselines: `--baseline` mode runs same tests via Swift UDS socket (`~/.aleph/desktop.sock`).

#### D. Stability (`test_stability.sh`)

```bash
# S1: Long-running
start_server_with_bridge
record_rss "before"
sleep 28800  # 8h (configurable)
record_rss "after"
assert_rss_growth_below 20%
assert_no_crash_logs

# S2: Hotkey cycles (requires cliclick or osascript)
for i in $(seq 1 100); do
  trigger_global_shortcut "Ctrl+Alt+/"
  sleep 0.3
  trigger_global_shortcut "Escape"
  sleep 0.3
done

# S4: High-frequency RPC
stress_rpc "desktop.screenshot" 10 300  # 10 req/s for 5min
```

### 2.3 Manual Verification Checklist

| # | Item | Method |
|---|------|--------|
| M1 | OCR Chinese accuracy | Compare Swift and Tauri OCR output on same Chinese screenshot |
| M2 | Canvas overlay visual quality | Visual inspection of transparency, position, A2UI patch rendering |
| M3 | Tray menu interaction | Manual operation: Provider switching, settings opening |
| M4 | Halo window experience | Hotkey response speed, position, animation smoothness |
| M5 | Settings page functionality | Modify settings in Leptos Control Plane, verify persistence |

### 2.4 Execution Order

```
1. Functional Parity (F1-F12)     <- Must all PASS
2. End-to-End Flow (E1-E6)        <- Must all PASS
3. Manual Verification (M1-M5)    <- Must all confirm
4. Performance Benchmark (P1-P5)  <- Record data, compare baseline
5. Stability (S1-S4)              <- Must all PASS
```

**Gate Rule**: Steps 1-3 all pass -> cleanup may begin (performance and stability can run in parallel).

---

## 3. Cleanup Checklist

### 3.1 Phase A — Remove Confirmed Unused Build Artifacts

| # | Item | Path | Precondition |
|---|------|------|-------------|
| C1 | UniFFI bindings (Swift) | `core/bindings/aleph.swift` | F1-F12 all PASS |
| C2 | UniFFI bindings (C header) | `core/bindings/alephFFI.h` | Same |
| C3 | Pre-compiled dylib | `apps/macos/Aleph/Frameworks/libalephcore.dylib` | Same |
| C4 | Generated FFI (Swift) | `apps/macos/Aleph/Sources/Generated/aleph.swift` | Same |
| C5 | Generated FFI (C header) | `apps/macos/Aleph/Sources/Generated/alephFFI.h` | Same |
| C6 | FFI Compatibility Layer | `apps/macos/Aleph/Sources/FFICompatibilityLayer.swift` | Same |

### 3.2 Phase B — Remove Swift macOS App

| # | Item | Path | Precondition |
|---|------|------|-------------|
| C7 | **Entire macOS App directory** | `apps/macos/` | E1-E6 + M1-M5 all pass |

Includes: 125+ Swift source files, DesktopBridge reference implementation, SwiftUI components, design system, Xcode project configuration, tests, DEPRECATED.md.

### 3.3 Phase C — Clean Residual References

| # | Action | Target | Precondition |
|---|--------|--------|-------------|
| C8 | Remove Swift references | `grep -r "apps/macos" --include="*.md" --include="*.toml" --include="*.yml"` | C7 done |
| C9 | Remove UniFFI references | `grep -r "uniffi\|alephFFI\|libalephcore" --include="*.rs" --include="*.toml"` | C1-C6 done |
| C10 | Remove XcodeGen references | `grep -r "xcodegen\|xcodeproj\|project.yml"` | C7 done |
| C11 | Clean Cargo.toml | Remove any `apps/macos` workspace member | C7 done |
| C12 | Clean .gitignore | Remove Swift/Xcode ignore rules if present | C7 done |

### 3.4 Phase D — Update Documentation

| # | Action | Target File | Content |
|---|--------|------------|---------|
| C13 | Update CLAUDE.md | `CLAUDE.md` | Remove `[DEPRECATED]` annotations, update project structure (delete `apps/macos`), update build commands, update environment section |
| C14 | Update architecture doc | `docs/reference/ARCHITECTURE.md` | Remove Swift macOS App sections |
| C15 | Update Phase 4 doc | `docs/plans/2026-02-25-phase4-cleanup.md` | Mark tasks as completed |
| C16 | Create migration record | `docs/plans/2026-02-25-macos-app-removal-record.md` | Record what was deleted, why, and acceptance results |

### 3.5 Phase E — Verify Cleanup Completeness

| # | Verification | Command |
|---|-------------|---------|
| C17 | Rust compiles clean | `cargo check --bin aleph-server --features control-plane` |
| C18 | Tauri compiles clean | `cd apps/desktop && cargo tauri build` |
| C19 | No dangling references | `grep -r "apps/macos\|libalephcore\|uniffi\|alephFFI" --include="*.rs" --include="*.toml" --include="*.md"` |
| C20 | Server + Bridge E2E works | Re-run E1-E3 |

### 3.6 Execution Order

```
Acceptance tests all pass
    |
    v
Phase A (C1-C6) -- Remove bindings/FFI
    |
    v
Phase B (C7) -- Remove apps/macos/
    |
    v
Phase C (C8-C12) -- Clean residual references
    |
    v
Phase D (C13-C16) -- Update documentation
    |
    v
Phase E (C17-C20) -- Verify cleanup completeness
    |
    v
Git commit: "cleanup: remove deprecated macOS Swift app and bindings"
```

---

## 4. Documentation Updates

### 4.1 CLAUDE.md Changes

**Remove:**
- `apps/macos/` entry with `[DEPRECATED]` in project structure
- macOS App build command (`cd apps/macos && xcodegen generate && open Aleph.xcodeproj`)
- Xcode-related environment notes (`Xcode generation`, `verify_swift_syntax.py`, `Xcode build cache cleanup`, `XCODEGEN_README.md` reference)

**Modify:**
- Project structure tree: `apps/` keeps only `cli/` and `desktop/`
- Tech stack table: remove `macOS App | Swift + SwiftUI + AppKit` row
- Desktop Bridge subsystem description: remove Swift mentions

**Add:**
- Note: "macOS Swift App completed migration and was removed on 2026-02-25, fully replaced by aleph-bridge (Tauri)"

### 4.2 Architecture Doc Changes

**`docs/reference/ARCHITECTURE.md`:**
- Remove Swift macOS App chapter/references
- Confirm `apps/desktop` (Tauri) as sole desktop client

### 4.3 Migration Completion Record

Create `docs/plans/2026-02-25-macos-app-removal-record.md`:

```markdown
# macOS Swift App Removal Record

## Date
2026-02-25

## Background
macOS Swift App deprecated since Server-Centric Build Architecture transition.
Tauri Bridge provides full functional parity.

## Acceptance Results
- Functional Parity (F1-F12): All PASS
- End-to-End Flow (E1-E6): All PASS
- Manual Verification (M1-M5): All confirmed
- Performance Benchmarks (P1-P5): [Record actual data]
- Stability (S1-S4): All PASS

## Deleted Content
- apps/macos/ (125+ Swift source files)
- core/bindings/aleph.swift, alephFFI.h
- apps/macos/Aleph/Frameworks/libalephcore.dylib
- All UniFFI/XcodeGen related references

## Preserved
- Git history retains all Swift code (traceable via git log)
- Design docs (docs/plans/2026-02-24-desktop-bridge-design.md etc.) retained as architectural decision records
```

### 4.4 Git Commit Strategy

```
cleanup: remove deprecated macOS Swift app and UniFFI bindings

- Remove apps/macos/ (125+ Swift source files)
- Remove core/bindings/ (UniFFI generated code)
- Remove libalephcore.dylib
- Update CLAUDE.md, ARCHITECTURE.md
- Add migration completion record

Tauri Bridge (apps/desktop) provides full functional parity.
Acceptance tests passed: F1-F12, E1-E6, M1-M5, P1-P5, S1-S4.
```

---

## Summary

| Dimension | Items | Gate |
|-----------|-------|------|
| Functional Parity | F1-F12 (12 items) | All PASS |
| End-to-End | E1-E6 (6 items) | All PASS |
| Manual Verification | M1-M5 (5 items) | All confirmed |
| Performance | P1-P5 (5 items) | Record & compare |
| Stability | S1-S4 (4 items) | All PASS |
| Cleanup | C1-C20 (20 items) | Sequential phases |
| **Total** | **52 items** | — |
