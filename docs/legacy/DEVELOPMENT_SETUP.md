# Development Setup

This document covers environment setup for Aether development.

## Python Environment

Aether uses `uv` for Python package management.

### macOS

```bash
# Python interpreter
~/.uv/python3/bin/python

# Activate virtual environment
source ~/.uv/python3/bin/activate

# Install packages
cd ~/.uv/python3 && uv pip install <package>
```

### Windows

```bash
# Python interpreter
C:\Users\zou\.uv\python3\Scripts\python.exe

# In Git Bash
/c/Users/zou/.uv/python3/Scripts/python.exe

# Install packages
cd C:\Users\zou\.uv\python3 && uv pip install <package>
```

## Xcode Setup (macOS)

```bash
# Generate Xcode project from project.yml
cd platforms/macos && xcodegen generate

# Open project
open Aether.xcodeproj
```

## Swift Syntax Validation

```bash
# Validate Swift syntax without full compilation
~/.uv/python3/bin/python Scripts/verify_swift_syntax.py <file.swift>
```

## Node.js Environment

Aether uses `fnm` (Fast Node Manager) for Node.js version management.

```bash
# fnm is auto-installed by Aether's runtime manager
# Node.js binaries are stored in ~/.aether/runtimes/fnm/
```

## Rust Toolchain

```bash
# Ensure Rust is installed
rustup --version

# Required Rust version (see rust-toolchain.toml)
rustup show

# Build and test
cd core && cargo build && cargo test
```

## Build Scripts

| Script | Description |
|--------|-------------|
| `./scripts/build-core.sh macos` | Build Rust core for macOS |
| `./scripts/build-macos.sh release` | Full macOS release build |
| `./scripts/generate-bindings.sh macos` | Generate UniFFI Swift bindings |

## IDE Recommendations

### VSCode Extensions

- rust-analyzer (Rust)
- Swift (Apple Swift)
- Tauri (Tauri development)

### Xcode Settings

- Enable "Show Invisibles" for whitespace visibility
- Set indentation to 4 spaces for Swift

## Troubleshooting

### UniFFI Binding Generation Fails

```bash
# Ensure library is built first
cargo build --release --features uniffi -p aethecore

# Generate bindings
cd core && cargo run --bin uniffi-bindgen generate \
    --library ../target/release/libaethecore.dylib \
    --language swift \
    --out-dir ../platforms/macos/Aether/Sources/Generated/
```

### Xcode Build Errors

1. Regenerate project: `cd platforms/macos && xcodegen generate`
2. Clean build folder: Cmd+Shift+K
3. Rebuild: Cmd+B

### Cargo Test Failures

```bash
# Run specific test
cargo test test_name -- --nocapture

# Run tests for specific module
cargo test extension::
```
