# Unified Build Pipeline Design

**Date**: 2026-02-27
**Status**: Approved
**Tool**: justfile (阶段化 recipe)

## Goal

为 Aleph 提供统一的本地构建流程，简化日常开发调试中 Server (含 Panel) 和 macOS Native App 的构建步骤。

## Architecture

### Three-Stage Pipeline

```
Stage 1: WASM (Panel UI)
   trunk build --release  →  dist/
   ↓
Stage 2: Server Binary
   cargo build --bin aleph-server --features control-plane --release
   (build.rs detects dist/, embeds WASM assets via rust-embed)
   ↓
Stage 3: macOS Native App
   cp server binary → Resources/
   xcodebuild (Release)  →  Aleph.app
```

- Stage 2 depends on Stage 1 output (`dist/`)
- Stage 3 depends on Stage 2 output (server binary)
- Each stage can be invoked independently

### Dependency Graph

```
just build
  ├── just server
  │     └── just wasm  (trunk build --release)
  │           → cargo build --bin aleph-server --features control-plane --release
  └── just macos
        └── _ensure-server (triggers just server if binary missing)
              → cp server → Resources/
              → xcodebuild
```

## Command Reference

### Daily Development

| Command | Purpose | Build mode |
|---------|---------|------------|
| `just dev` | Run server with Panel UI | debug |
| `just dev-no-panel` | Run server without Panel UI | debug |

### Release Builds

| Command | Purpose | Stages triggered |
|---------|---------|------------------|
| `just build` | Full build: Server + macOS App | 1 → 2 → 3 |
| `just server` | Server binary with Panel | 1 → 2 |
| `just server-debug` | Server binary with Panel (fast) | 1 → 2 (debug) |
| `just macos` | macOS native app | (auto 1 → 2) → 3 |

### Single Stage

| Command | Purpose |
|---------|---------|
| `just wasm` | Only build WASM Panel UI |
| `just xcode` | Only run Xcode build (assumes server exists) |

### Utilities

| Command | Purpose |
|---------|---------|
| `just clean` | Clean all build artifacts |
| `just check` | cargo check workspace |
| `just test` | cargo test workspace |
| `just install-deps` | Verify build dependencies installed |

## Key Variables

```
server_bin      = aleph-server
release_dir     = target/release
wasm_dist       = core/ui/control_plane/dist
macos_project   = apps/macos-native
macos_build_dir = apps/macos-native/build/Build/Products/Release
```

## Implementation

Single file: `justfile` at project root. No additional scripts needed.

### WASM Build

- Uses `trunk build --release` (existing Trunk.toml config)
- Output: `core/ui/control_plane/dist/`
- build.rs caching: skips rebuild if dist/ exists

### Server Build

- `cargo build --bin aleph-server --features control-plane --release`
- build.rs auto-invokes trunk, rust-embed embeds dist/ into binary
- Output: `target/release/aleph-server`

### macOS Build

- Copies server binary to `apps/macos-native/Resources/`
- Runs `xcodegen generate` (if available) then `xcodebuild`
- Output: `Aleph.app` in derived data

## Non-Goals

- CI/CD automation (existing GitHub Actions workflows cover this)
- Cross-compilation for Linux/Windows (use CI for those)
- Tauri desktop builds (future work if needed)
