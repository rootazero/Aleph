# Aether Build Commands

This document provides all build commands for the Aether project.

## Quick Reference

| Task | Command |
|------|---------|
| Build Rust (dev) | `cd core && cargo build` |
| Build Rust (release) | `cd core && cargo build --release` |
| Run tests | `cd core && cargo test` |
| Generate Xcode project | `cd platforms/macos && xcodegen generate` |
| Build macOS app | `xcodebuild -project Aether.xcodeproj -scheme Aether build` |
| Run Tauri dev | `cd platforms/tauri && pnpm tauri dev` |
| Build Tauri release | `cd platforms/tauri && pnpm tauri build` |

## Building Rust Core

```bash
# From repository root
cd core/

# Development build (default: UniFFI for macOS)
cargo build

# Release build
cargo build --release

# Generate UniFFI bindings (macOS)
cargo run --bin uniffi-bindgen generate \
  --library target/release/libaethecore.dylib \
  --language swift \
  --out-dir ../platforms/macos/Aether/Sources/Generated/
```

## Building macOS Client (Native)

```bash
cd platforms/macos/
xcodegen generate                  # Generate Xcode project
open Aether.xcodeproj              # Open in Xcode
# Or:
xcodebuild -project Aether.xcodeproj -scheme Aether -configuration Release build
```

## Building Tauri Client (Cross-platform)

```bash
cd platforms/tauri/

# Install dependencies
pnpm install

# Development mode
pnpm tauri dev

# Build release
pnpm tauri build

# Build for specific platform (from any OS with cross-compilation)
pnpm tauri build --target x86_64-pc-windows-msvc
pnpm tauri build --target x86_64-unknown-linux-gnu
```

## Using Build Scripts

```bash
# Build core for macOS
./scripts/build-core.sh macos

# Build macOS app
./scripts/build-macos.sh release
```

## Testing

```bash
cd core/
cargo test                         # All tests
cargo test router                  # Module-specific tests
cargo test --workspace             # All workspace tests
```

## Feature Flags

The Rust core uses feature flags to control platform-specific builds:

| Feature | Platform | Description |
|---------|----------|-------------|
| `uniffi` | macOS | Generates Swift bindings via UniFFI (default) |

```bash
# macOS build (default)
cargo build --features uniffi
```

---

## Archived: Windows Native

> **Note**: The Windows native platform (C#/WinUI 3) has been archived.
> For Windows support, use the Tauri cross-platform build instead.

See `platforms/windows/ARCHIVED.md` for details.
