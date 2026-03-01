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
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p {{wasm_dist}}
    # 1. Tailwind CSS
    (cd {{wasm_dir}} && npm run build:css)
    # 2. Compile Rust → WASM
    cargo build -p aleph-control-plane --target wasm32-unknown-unknown --release
    # 3. Generate JS bindings
    wasm-bindgen --target web --no-typescript \
        --out-dir {{wasm_dist}} --out-name aleph_dashboard \
        target/wasm32-unknown-unknown/release/aleph_dashboard.wasm
    # 4. Runtime index.html
    cat > {{wasm_dist}}/index.html << 'HTMLEOF'
    <!DOCTYPE html>
    <html lang="en">
      <head>
        <meta charset="utf-8" />
        <meta name="viewport" content="width=device-width, initial-scale=1" />
        <title>Aleph Dashboard</title>
        <link rel="stylesheet" href="/tailwind.css" />
      </head>
      <body class="bg-surface text-text-primary">
        <noscript>This application requires JavaScript to run.</noscript>
        <script type="module">
          import init from '/aleph_dashboard.js';
          await init('/aleph_dashboard_bg.wasm');
        </script>
      </body>
    </html>
    HTMLEOF
    echo "✓ WASM: {{wasm_dist}}/"

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

# Run proptest with high coverage (1024 cases per test)
test-proptest:
    PROPTEST_CASES=1024 cargo test --workspace --lib

# Run loom concurrency tests
test-loom:
    RUSTFLAGS="--cfg loom" LOOM_MAX_PREEMPTIONS=3 cargo test --features loom --lib

# Run full logic review suite (proptest + loom)
test-logic: test-proptest test-loom

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
        just server
    else
        echo "✓ Server binary exists: {{release_dir}}/{{server_bin}}"
    fi
