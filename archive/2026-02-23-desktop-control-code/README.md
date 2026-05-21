# Desktop Control Code Archive

Archived: 2026-02-23
Source commit: 58333cb6 (pre-purification)
Purification PR: worktree-server-purification

## What's Here

Desktop control components removed from Aleph during the "Server Purification" refactoring.
These files were working code at the time of archival but depend on crate dependencies
that have also been removed (chromiumoxide, accessibility-sys, core-foundation, core-graphics).

## Module Inventory

| Module | Files | Description |
|--------|-------|-------------|
| `perception/` | 19 | AX tree capture, screen capture, input simulation, State Bus, PAL |
| `vision/` | 4 | OCR / vision analysis service |
| `browser/` | 10 | Chrome DevTools Protocol (CDP) automation via chromiumoxide |
| `builtin_tools/canvas/` | 7 | A2UI protocol, WebView rendering, canvas controller |
| `builtin_tools/snapshot_capture.rs` | 1 | AX tree + OCR snapshot tool |
| `gateway/handlers/` | 3 | browser.*, state_bus.*, ocr.* RPC handlers |
| `gateway/reverse_rpc.rs` | 1 | Server-to-Client reverse RPC |
| `gateway/client_manifest.rs` | 1 | Client capability declaration |
| `executor/router.rs` | 1 | Tool routing decision engine |
| `executor/routed_executor.rs` | 1 | Routed execution wrapper |
| `dispatcher/types/execution_policy.rs` | 1 | ServerOnly/ClientOnly/PreferServer/PreferClient enum |
| `tests/` | 6 | Perception BDD features + integration tests |

## Reuse Notes

- Perception PAL traits (`pal/sensor.rs`, `pal/actuator.rs`) define clean cross-platform abstractions
- Browser CDP module is a full-featured wrapper around chromiumoxide
- Canvas A2UI protocol is a general-purpose agent-to-UI rendering system
- State Bus implements JSON Patch (RFC 6902) I-Frame + P-Frame streaming
