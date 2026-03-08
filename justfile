# Aleph Build Pipeline
# Usage: just <recipe>    Run: just --list

set shell := ["bash", "-euo", "pipefail", "-c"]

# ─── Variables ───
release_dir     := "target/release"
debug_dir       := "target/debug"
panel_dir       := "apps/panel"
panel_dist      := "apps/panel/dist"
macos_dir       := "apps/macos-native"
macos_resources := "apps/macos-native/Aleph/Resources"
macos_app       := "apps/macos-native/build/Build/Products/Release/Aleph.app"
server_bin      := "aleph"

# ─── Default ───

# Show available recipes
default:
    @just --list

# ─── Daily Development ───

# Run server (debug, rebuilds WASM first)
dev: wasm
    cargo run -p alephcore --bin {{server_bin}}

# ─── Full Builds ───

# Full build: WASM → Server → macOS App (release)
all: build macos

# Build server (release)
build: wasm
    cargo build -p alephcore --bin {{server_bin}} --release
    @echo "✓ Server: {{release_dir}}/{{server_bin}}"

# Build server (debug, faster compile)
build-debug: wasm
    cargo build -p alephcore --bin {{server_bin}}
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
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p {{panel_dist}}
    # 1. Tailwind CSS
    (cd {{panel_dir}} && npm run build:css)
    # 2. Compile Rust → WASM
    cargo build -p aleph-panel --target wasm32-unknown-unknown --release
    # 3. Generate JS bindings
    wasm-bindgen --target web --no-typescript \
        --out-dir {{panel_dist}} --out-name aleph_panel \
        target/wasm32-unknown-unknown/release/aleph_panel.wasm
    # 4. Runtime index.html
    cat > {{panel_dist}}/index.html << 'HTMLEOF'
    <!DOCTYPE html>
    <html lang="en">
      <head>
        <meta charset="utf-8" />
        <meta name="viewport" content="width=device-width, initial-scale=1" />
        <title>Aleph Panel</title>
        <link rel="stylesheet" href="/tailwind.css" />
      </head>
      <body class="bg-surface text-text-primary">
        <noscript>This application requires JavaScript to run.</noscript>
        <script type="module">
          import init from '/aleph_panel.js';
          await init('/aleph_panel_bg.wasm');
        </script>
      </body>
    </html>
    HTMLEOF
    echo "✓ WASM: {{panel_dist}}/"

# Run Xcode build only (assumes server binary exists in Resources)
xcode:
    cd {{macos_dir}} && xcodebuild \
        -project Aleph.xcodeproj \
        -scheme Aleph \
        -configuration Release \
        -derivedDataPath build \
        build
    @echo "✓ Xcode: {{macos_app}}"

# ─── Testing ───

# Quick check: core compiles
check:
    cargo check -p alephcore

# Quick check: desktop crate compiles
check-desktop:
    cargo check -p aleph-desktop

# Run core tests
test:
    cargo test -p alephcore --lib

# Run desktop crate tests
test-desktop:
    cargo test -p aleph-desktop --lib

# Run desktop integration tests
test-desktop-integration:
    cargo test -p alephcore --lib builtin_tools::desktop

# Run all desktop-related tests
test-desktop-all: test-desktop test-desktop-integration

# Run proptest with high coverage (1024 cases per test)
test-proptest:
    PROPTEST_CASES=1024 cargo test -p alephcore --lib

# Run loom concurrency tests
test-loom:
    LOOM_MAX_PREEMPTIONS=3 cargo test -p alephcore --features loom --lib loom

# Run full logic review suite (proptest + loom)
test-logic: test-proptest test-loom

# Run all tests (core + desktop + proptest)
test-all: test test-desktop-all test-proptest

# ─── Lint ───

# Clippy on core
clippy:
    cargo clippy -p alephcore -- -D warnings

# Clippy on desktop crate
clippy-desktop:
    cargo clippy -p aleph-desktop -- -D warnings

# Clippy everything
clippy-all: clippy clippy-desktop

# ─── Utilities ───

# Clean all build artifacts
clean:
    cargo clean
    rm -rf {{panel_dist}}
    rm -rf {{macos_dir}}/build
    rm -rf {{macos_resources}}/{{server_bin}}
    @echo "✓ Cleaned"

# Verify build dependencies are installed
deps:
    #!/usr/bin/env bash
    ok=true
    for cmd in cargo wasm-bindgen npm xcodegen xcodebuild; do
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
        just build
    else
        echo "✓ Server binary exists: {{release_dir}}/{{server_bin}}"
    fi
