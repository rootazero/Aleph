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

## Building Rust Core

```bash
# From repository root
cd core/

# Development build (default: UniFFI for macOS)
cargo build

# Release build
cargo build --release

# Build for Windows (C ABI)
cargo build --release --no-default-features --features cabi

# Generate UniFFI bindings (macOS)
cargo run --bin uniffi-bindgen generate \
  --library target/release/libaethecore.dylib \
  --language swift \
  --out-dir ../platforms/macos/Aether/Sources/Generated/
```

## Building macOS Client

```bash
cd platforms/macos/
xcodegen generate                  # Generate Xcode project
open Aether.xcodeproj              # Open in Xcode
# Or:
xcodebuild -project Aether.xcodeproj -scheme Aether -configuration Release build
```

## Building Windows Client (on Windows)

```powershell
cd platforms/windows/
dotnet build -c Release
```

## Using Build Scripts

```bash
# Build core for all platforms
./scripts/build-core.sh all

# Build macOS app
./scripts/build-macos.sh release

# Build Windows app (on Windows)
.\scripts\build-windows.ps1 -Config Release
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
| `cabi` | Windows | Generates C ABI for csbindgen P/Invoke |

```bash
# macOS build (default)
cargo build --features uniffi

# Windows build
cargo build --no-default-features --features cabi
```
