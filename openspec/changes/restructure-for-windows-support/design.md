# Design: Monorepo Architecture for Cross-Platform Support

## Overview

This document describes the architectural decisions for restructuring Aether from a macOS-centric layout to a cross-platform Monorepo. The design prioritizes backward compatibility for macOS while establishing clear patterns for Windows (and future Linux) support.

## Architecture Diagram

```
aether/                              # Repository Root
├── .github/
│   └── workflows/
│       ├── rust-core.yml            # Rust core CI (tests, lints)
│       ├── macos-app.yml            # macOS app build
│       └── windows-app.yml          # Windows app build
│
├── core/                            # 🦀 Shared Rust Core
│   ├── Cargo.toml                   # Workspace member
│   ├── build.rs                     # UniFFI + csbindgen generation
│   ├── uniffi.toml                  # UniFFI config
│   ├── src/
│   │   ├── lib.rs                   # Main entry (unchanged)
│   │   ├── aether.udl               # UniFFI interface
│   │   ├── ffi/
│   │   │   ├── mod.rs
│   │   │   ├── uniffi_exports.rs    # macOS UniFFI (existing)
│   │   │   └── cabi_exports.rs      # Windows C ABI (new)
│   │   └── ...                      # All existing modules
│   └── tests/
│
├── platforms/                       # 📱 Platform-Specific Code
│   │
│   ├── macos/                       # 🍎 macOS Application
│   │   ├── project.yml              # XcodeGen (updated paths)
│   │   ├── Aether/
│   │   │   ├── Sources/
│   │   │   │   ├── Generated/       # UniFFI bindings (aether.swift)
│   │   │   │   └── ...              # All existing Swift code
│   │   │   ├── Frameworks/
│   │   │   │   └── libaethecore.dylib
│   │   │   └── Resources/
│   │   ├── AetherTests/
│   │   └── AetherUITests/
│   │
│   └── windows/                     # 🪟 Windows Application (Placeholder)
│       ├── Aether.sln               # Visual Studio solution
│       ├── Aether/
│       │   ├── Aether.csproj        # C# project
│       │   ├── App.xaml             # WinUI 3 entry
│       │   ├── Interop/
│       │   │   └── NativeMethods.g.cs  # csbindgen output
│       │   └── libs/
│       │       └── aethecore.dll
│       └── Aether.Tests/
│
├── shared/                          # 📦 Cross-Platform Resources
│   ├── config/
│   │   └── default-config.toml      # Default configuration
│   ├── locales/
│   │   ├── en.json                  # Master locale files
│   │   └── zh-Hans.json
│   └── docs/                        # Shared documentation
│
├── scripts/                         # 🔧 Build Scripts
│   ├── build-core.sh                # Build Rust (multi-target)
│   ├── build-macos.sh               # macOS full build
│   ├── build-windows.ps1            # Windows full build
│   ├── generate-bindings.sh         # FFI binding generation
│   └── sync-locales.py              # Locale conversion utility
│
├── Cargo.toml                       # Workspace root
├── CLAUDE.md                        # Updated project guide
├── VERSION                          # Single version source
└── README.md
```

## Key Design Decisions

### 1. FFI Strategy: UniFFI vs csbindgen

| Platform | FFI Tool | Rationale |
|----------|----------|-----------|
| macOS | UniFFI | Existing integration, Swift-native bindings, async support |
| Windows | csbindgen | C# P/Invoke generation, WinUI 3 compatible, no runtime |

**UniFFI (macOS)** - Continues current approach:
```rust
// lib.rs
#[cfg(feature = "uniffi")]
uniffi::include_scaffolding!("aether");
```

**csbindgen (Windows)** - New C ABI exports:
```rust
// ffi/cabi_exports.rs
#[cfg(feature = "cabi")]
#[no_mangle]
pub extern "C" fn aether_init(config_path: *const c_char) -> i32 { ... }

#[cfg(feature = "cabi")]
#[no_mangle]
pub extern "C" fn aether_process(
    input: *const c_char,
    callback: extern "C" fn(*const c_char),
) -> i32 { ... }
```

### 2. Feature Flag Design

```toml
# core/Cargo.toml
[features]
default = []
uniffi = ["dep:uniffi"]     # macOS: cargo build --features uniffi
cabi = []                   # Windows: cargo build --features cabi

[dependencies]
uniffi = { version = "0.28", features = ["cli"], optional = true }

[build-dependencies]
uniffi = { version = "0.28", features = ["build"], optional = true }
csbindgen = { version = "1.9", optional = true }
```

Build selection:
- macOS: `cargo build --release --features uniffi`
- Windows: `cargo build --release --features cabi --target x86_64-pc-windows-msvc`

### 3. Callback Pattern Abstraction

Both platforms use callbacks for Rust → UI communication. The core defines an abstract pattern:

```rust
// ffi/callback_bridge.rs (shared)
pub trait PlatformCallbackBridge: Send + Sync {
    fn on_processing_state_changed(&self, state: ProcessingState);
    fn on_streaming_text(&self, chunk: String);
    fn on_error(&self, error: AetherError);
}

// ffi/uniffi_exports.rs (macOS)
#[cfg(feature = "uniffi")]
#[derive(uniffi::Object)]
pub struct UniffiCallbackBridge {
    handler: Arc<dyn AetherEventHandler>,
}

// ffi/cabi_exports.rs (Windows)
#[cfg(feature = "cabi")]
#[repr(C)]
pub struct CabiCallbackBridge {
    on_state_changed: extern "C" fn(i32),
    on_streaming_text: extern "C" fn(*const c_char),
    on_error: extern "C" fn(*const c_char, i32),
}
```

### 4. Workspace Configuration

```toml
# /Cargo.toml (root)
[workspace]
resolver = "2"
members = ["core"]

[workspace.package]
version = "0.1.0"
edition = "2021"
license = "MIT"
repository = "https://github.com/user/aether"
rust-version = "1.92"

[workspace.dependencies]
# Shared dependencies defined once
tokio = { version = "1.35", features = ["rt-multi-thread", "sync", "time", "macros", "process", "fs", "io-util"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
thiserror = "1.0"
# ... all other shared deps
```

### 5. Version Management

Single source of truth in `/VERSION`:
```
0.1.0
```

Consumed by:
- `Cargo.toml`: Read at build time or via `version.workspace = true`
- `project.yml` (macOS): XcodeGen reads from file
- `.csproj` (Windows): MSBuild reads from file

### 6. Localization Strategy

Master files in `shared/locales/` with platform-specific conversion:

```python
# scripts/sync-locales.py
def convert_to_strings(json_path, output_path):
    """Convert JSON to macOS .strings format"""
    ...

def convert_to_resx(json_path, output_path):
    """Convert JSON to Windows .resx format"""
    ...
```

### 7. CI/CD Pipeline Design

**Path-based triggers**:
```yaml
# .github/workflows/rust-core.yml
on:
  push:
    paths:
      - 'core/**'
      - 'Cargo.toml'
      - '.github/workflows/rust-core.yml'
```

**Artifact sharing**:
```yaml
# rust-core.yml produces artifacts
- uses: actions/upload-artifact@v4
  with:
    name: libaethecore-${{ matrix.platform }}
    path: core/target/release/libaethecore.*

# macos-app.yml consumes artifacts
- uses: actions/download-artifact@v4
  with:
    name: libaethecore-macos
```

## Migration Path Details

### Step 1: Add Workspace (Non-Breaking)

Create `/Cargo.toml` that wraps existing `Aether/core/`:
```toml
[workspace]
members = ["Aether/core"]
```

This allows testing workspace build without moving files.

### Step 2: Move Core

```bash
git mv Aether/core core
# Update Cargo.toml members = ["core"]
```

### Step 3: Move macOS App

```bash
mkdir -p platforms/macos
git mv Aether platforms/macos/Aether
git mv AetherTests platforms/macos/AetherTests
git mv AetherUITests platforms/macos/AetherUITests
git mv project.yml platforms/macos/project.yml
```

### Step 4: Update References

**project.yml paths**:
```yaml
# Before
- path: Aether/Sources
- framework: Aether/Frameworks/libaethecore.dylib

# After
- path: platforms/macos/Aether/Sources
- framework: platforms/macos/Aether/Frameworks/libaethecore.dylib
```

**Build script paths**:
```bash
# Before
cd "${PROJECT_DIR}/Aether/core"

# After
cd "${PROJECT_DIR}/../../core"
```

## Trade-offs

### Chosen: Monorepo with Workspace
**Pros**:
- Atomic commits across core + platforms
- Single PR for cross-cutting changes
- Shared CI configuration
- Version synchronization

**Cons**:
- Larger checkout size
- Platform teams see all code
- Complex path management

### Rejected: Separate Repositories
**Pros**:
- Clean separation
- Independent versioning

**Cons**:
- Difficult to keep in sync
- Cross-repo PRs awkward
- CI complexity for core changes

### Rejected: Keep Current Structure
**Pros**:
- No migration work

**Cons**:
- Windows support requires fundamental changes anyway
- Technical debt accumulates

## Security Considerations

1. **Build isolation**: Windows DLL built on Windows runners only
2. **Code signing**: Per-platform signing in platform-specific workflows
3. **Dependency auditing**: `cargo audit` at workspace level catches all
4. **No secrets in shared code**: API keys remain per-platform

## Testing Strategy

1. **Pre-migration**: Full macOS test suite passes
2. **Post-migration**: Same test suite passes from new location
3. **Workspace tests**: `cargo test --workspace` at root
4. **CI verification**: Both platforms build successfully
