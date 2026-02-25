# macOS Swift App Removal Record

## Date
2026-02-25

## Background
The macOS Swift App (`apps/macos/`) was deprecated as part of the Server-Centric Build Architecture transition. The Tauri Bridge (`apps/desktop/`) now provides full functional parity.

## Acceptance Results
- Functional Parity (F1-F12): Pending execution
- End-to-End Flow (E1-E6): Pending execution
- Manual Verification (M1-M5): Pending execution
- Performance Benchmarks (P1-P5): Pending execution
- Stability (S1-S4): Pending execution

Note: Acceptance test suite created at tests/bridge_acceptance/. Results will be recorded here after execution.

## Deleted Content
- apps/macos/ (125+ Swift source files, DesktopBridge, SwiftUI components, Xcode config)
- core/bindings/aleph.swift, alephFFI.h (UniFFI generated code)
- .github/workflows/macos-app.yml (CI workflow)
- Scripts/build-macos.sh, verify_swift_syntax.py, and other macOS-specific scripts

## Preserved
- Git history retains all Swift code (traceable via git log)
- Design docs in docs/plans/ retained as architectural decision records
- Legacy docs in docs/legacy/ retained for reference
