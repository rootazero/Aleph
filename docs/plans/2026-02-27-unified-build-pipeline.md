# Unified Build Pipeline Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Create a single `justfile` at project root that provides one-command builds for Server (with Panel) and macOS Native App.

**Architecture:** Three-stage pipeline (WASM → Server → macOS App) with dependency chains. Each stage is an independent just recipe that can be invoked alone or chained.

**Tech Stack:** just (command runner), trunk (WASM), cargo, xcodegen, xcodebuild

---

### Task 1: Create justfile

**Files:**
- Create: `justfile`

**Step 1: Write the justfile**

```just
# Aleph Build Pipeline
# Usage: just <recipe>    Run: just --list

set shell := ["bash", "-euo", "pipefail", "-c"]

# ─── Variables ───
release_dir     := "target/release"
debug_dir       := "target/debug"
wasm_dir        := "core/ui/control_plane"
wasm_dist       := "core/ui/control_plane/dist"
macos_dir       := "apps/macos-native"
macos_resources := "apps/macos-native/Aleph/Resources"
macos_app       := "apps/macos-native/build/Build/Products/Release/Aleph.app"
server_bin      := "aleph-server"

# ─── Default ───

# Show available recipes
default:
    @just --list

# ─── Daily Development ───

# Run server with Panel UI (debug)
dev:
    cargo run --bin {{server_bin}} --features control-plane

# Run server without Panel UI (debug)
dev-no-panel:
    cargo run --bin {{server_bin}}

# ─── Full Builds ───

# Full build: WASM → Server → macOS App (release)
build: server macos

# Build server binary with Panel (release)
server: wasm
    cargo build --bin {{server_bin}} --features control-plane --release
    @echo "✓ Server: {{release_dir}}/{{server_bin}}"

# Build server binary with Panel (debug, faster compile)
server-debug: wasm
    cargo build --bin {{server_bin}} --features control-plane
    @echo "✓ Server (debug): {{debug_dir}}/{{server_bin}}"

# Build macOS native app (release)
macos: _ensure-server
    mkdir -p {{macos_resources}}
    cp {{release_dir}}/{{server_bin}} {{macos_resources}}/{{server_bin}}
    cd {{macos_dir}} && xcodegen generate
    cd {{macos_dir}} && xcodebuild \
        -project Aleph.xcodeproj \
        -scheme Aleph \
        -configuration Release \
        -derivedDataPath build \
        build
    @echo "✓ macOS App: {{macos_app}}"

# ─── Single Stage ───

# Build WASM Panel UI only
wasm:
    cd {{wasm_dir}} && trunk build --release
    @echo "✓ WASM: {{wasm_dist}}/"

# Run Xcode build only (assumes server binary exists in Resources)
xcode:
    cd {{macos_dir}} && xcodebuild \
        -project Aleph.xcodeproj \
        -scheme Aleph \
        -configuration Release \
        -derivedDataPath build \
        build
    @echo "✓ Xcode: {{macos_app}}"

# ─── Utilities ───

# Clean all build artifacts
clean:
    cargo clean
    rm -rf {{wasm_dist}}
    rm -rf {{macos_dir}}/build
    rm -rf {{macos_resources}}/{{server_bin}}
    @echo "✓ Cleaned"

# Quick workspace compile check
check:
    cargo check --workspace

# Run all tests
test:
    cargo test --workspace

# Verify build dependencies are installed
deps:
    #!/usr/bin/env bash
    ok=true
    for cmd in cargo trunk wasm-bindgen xcodegen xcodebuild; do
        if command -v "$cmd" &>/dev/null; then
            printf "  ✓ %-16s %s\n" "$cmd" "$(which $cmd)"
        else
            printf "  ✗ %-16s missing\n" "$cmd"
            ok=false
        fi
    done
    $ok || { echo ""; echo "Install missing deps before building."; exit 1; }

# ─── Internal ───

[private]
_ensure-server:
    #!/usr/bin/env bash
    if [ ! -f {{release_dir}}/{{server_bin}} ]; then
        echo "Server binary not found, building..."
        just server
    else
        echo "✓ Server binary exists: {{release_dir}}/{{server_bin}}"
    fi
```

**Step 2: Verify justfile syntax**

Run: `just --list`
Expected: All recipes listed without parse errors.

**Step 3: Smoke test `just check`**

Run: `just check`
Expected: `cargo check --workspace` runs and passes.

**Step 4: Smoke test `just deps`**

Run: `just deps`
Expected: Shows installed/missing dependencies with checkmarks.

**Step 5: Commit**

```bash
git add justfile
git commit -m "build: add justfile for unified build pipeline"
```

---

### Task 2: Update design doc with final paths

**Files:**
- Modify: `docs/plans/2026-02-27-unified-build-pipeline-design.md`

**Step 1: Fix macOS Resources path**

The design doc says `apps/macos-native/Resources/` but the actual path (matching CI and Swift code) is `apps/macos-native/Aleph/Resources/`. Update the design doc to reflect the correct path.

**Step 2: Commit**

```bash
git add docs/plans/2026-02-27-unified-build-pipeline-design.md
git commit -m "docs: fix macOS Resources path in build pipeline design"
```
