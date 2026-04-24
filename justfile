# Aleph Build Pipeline
# Usage: just <recipe>    Run: just --list

set shell := ["bash", "-euo", "pipefail", "-c"]

# ─── Variables ───
release_dir     := "target/release"
debug_dir       := "target/debug"
panel_dir       := "interfaces/webchat"
panel_dist      := "interfaces/webchat/dist"
server_bin      := "aleph-server"

# ─── Default ───

# Show available recipes
default:
    @just --list

# ─── Daily Development ───

# Run server (debug, rebuilds WASM first)
dev: wasm
    cargo run -p alephcore --bin {{server_bin}}

# ─── Full Builds ───

# Full build: WASM → Swift Bridge → Server (release)
all: build

# Build Swift bridge (macOS only)
swift-bridge:
    cd desktop/macos/bridge && swift build -c release
    mkdir -p {{release_dir}} {{debug_dir}}
    ln -sf "$PWD/desktop/macos/bridge/.build/release/AlephBridge" {{release_dir}}/aleph-bridge
    ln -sf "$PWD/desktop/macos/bridge/.build/release/AlephBridge" {{debug_dir}}/aleph-bridge
    @echo "✓ Swift bridge: desktop/macos/bridge/.build/release/AlephBridge"

# Build server (release)
build: wasm swift-bridge
    cargo build -p alephcore --bin {{server_bin}} --release
    @echo "✓ Server: {{release_dir}}/{{server_bin}}"

# Build server (debug, faster compile)
build-debug: wasm
    cargo build -p alephcore --bin {{server_bin}}
    @echo "✓ Server (debug): {{debug_dir}}/{{server_bin}}"

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
          await init({ module_or_path: '/aleph_panel_bg.wasm' });
        </script>
      </body>
    </html>
    HTMLEOF
    echo "✓ WASM: {{panel_dist}}/"

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

# Run desktop-macos crate tests
test-desktop-macos:
    cargo test -p aleph-desktop-macos --lib

# Run desktop integration tests
test-desktop-integration:
    cargo test -p alephcore --lib builtin_tools::desktop

# Run all desktop-related tests
test-desktop-all: test-desktop test-desktop-macos test-desktop-integration

# Run proptest with high coverage (1024 cases per test)
test-proptest:
    PROPTEST_CASES=1024 cargo test -p alephcore --lib

# Run loom concurrency tests
test-loom:
    LOOM_MAX_PREEMPTIONS=3 cargo test -p alephcore --features loom --lib loom

# Run full logic review suite (proptest + loom)
test-logic: test-proptest test-loom

# Run all tests (core + desktop + proptest)
test-all: test test-desktop-all test-proptest check-phase5

# Phase 5 exit criterion 9 gate.
check-phase5:
    ./scripts/check-phase5-exit.sh

# ─── Lint ───

# Clippy on core
clippy:
    cargo clippy -p alephcore -- -D warnings

# Clippy on desktop crate
clippy-desktop:
    cargo clippy -p aleph-desktop -- -D warnings

# Clippy on desktop-macos crate
clippy-desktop-macos:
    cargo clippy -p aleph-desktop-macos -- -D warnings

# Clippy everything
clippy-all: clippy clippy-desktop clippy-desktop-macos

# ─── Utilities ───

# Clean all build artifacts
clean:
    cargo clean
    rm -rf {{panel_dist}}
    @echo "✓ Cleaned"

# Verify session store migration status (legacy SQLite vs file backend)
migrate-verify:
    #!/usr/bin/env bash
    set -euo pipefail
    DATA_DIR="${ALEPH_DATA_DIR:-$HOME/.aleph}"
    DB="$DATA_DIR/sessions.db"
    SESSIONS_DIR="$DATA_DIR/sessions"
    MARKER="$SESSIONS_DIR/.migrated_from_sqlite"

    echo "=== Session Store Migration Report ==="
    echo "Data dir: $DATA_DIR"

    if [[ -f "$DB" ]]; then
        LEGACY_COUNT=$(sqlite3 "$DB" "SELECT COUNT(*) FROM messages;" 2>/dev/null || echo "0")
        echo "Legacy SQLite messages: $LEGACY_COUNT"
    else
        echo "Legacy SQLite DB not found."
    fi

    if [[ -d "$SESSIONS_DIR" ]]; then
        JSONL_COUNT=$(find "$SESSIONS_DIR" -name "*.jsonl" -not -path "*/.archive/*" | wc -l | tr -d ' ')
        echo "JSONL transcript files: $JSONL_COUNT"
    else
        echo "JSONL session directory not found."
        JSONL_COUNT=0
    fi

    if [[ -f "$MARKER" ]]; then
        echo "Migration marker: present ($(cat "$MARKER"))"
    else
        echo "Migration marker: not present"
    fi

    echo "======================================"

# Verify build dependencies are installed
deps:
    #!/usr/bin/env bash
    ok=true
    for cmd in cargo wasm-bindgen npm swift; do
        if command -v "$cmd" &>/dev/null; then
            printf "  ✓ %-16s %s\n" "$cmd" "$(which $cmd)"
        else
            printf "  ✗ %-16s missing\n" "$cmd"
            ok=false
        fi
    done
    $ok || { echo ""; echo "Install missing deps before building."; exit 1; }

# ─── Release ───

# Release a new version: bump VERSION, commit, push, trigger workflow
# CHANGELOG.md should be written by AI (Claude) BEFORE running this command.
# Usage: just release 2026.03.29
release version:
    #!/usr/bin/env bash
    set -euo pipefail
    VERSION="{{version}}"

    # Verify CHANGELOG.md has an entry for this version
    if ! grep -q "## \[${VERSION}\]" CHANGELOG.md 2>/dev/null; then
        echo "Error: No changelog entry found for [${VERSION}] in CHANGELOG.md"
        echo "Write the changelog first, then run: just release ${VERSION}"
        exit 1
    fi

    # Update VERSION file
    echo "$VERSION" > VERSION

    # Stage, commit, push
    git add -f VERSION CHANGELOG.md
    git commit -m "release: v${VERSION}"
    git push origin main

    # Delete old release if exists
    gh release delete "v${VERSION}" --yes 2>/dev/null || true
    git push origin ":refs/tags/v${VERSION}" 2>/dev/null || true

    # Trigger workflow
    gh workflow run aleph-server-release.yml

    echo ""
    echo "✓ Release v${VERSION} triggered!"
    echo "  Monitor: gh run list --limit 1"

# ─── Integration Probes ───

# Provider config integration probes (Layer 1 + 2)
test-probes:
    cargo test --test provider_config_probe --test provider_rpc_probe -p alephcore -- --test-threads=1

# Playwright E2E tests (Layer 3) — requires `just wasm` for UI tests
test-e2e:
    npx playwright test --project=chromium

# ─── Desktop Bridge (codex-inspired JSON-RPC helper) ───

# Dump Rust-side desktop-bridge schemas to JSON (source of truth for Swift golden tests)
bridge-schema:
    @mkdir -p desktop/macos/bridge/Tests/AlephBridgeTests/Fixtures
    cargo run -p aleph-protocol --bin export_desktop_bridge_schema \
        > desktop/macos/bridge/Tests/AlephBridgeTests/Fixtures/schema.json
    @echo "✓ schema.json written to desktop/macos/bridge/Tests/AlephBridgeTests/Fixtures/"

# Run Swift-side bridge unit tests (golden fixtures, codec, router)
bridge-test:
    cd desktop/macos/bridge && swift test

# End-to-end: build Swift helper, then run ignored Rust e2e tests against it
test-bridge-e2e: swift-bridge
    cargo test -p aleph-desktop-macos --test bridge_e2e -- --ignored --nocapture

# End-to-end: camera snap/clip via the Swift helper. Requires camera permission.
test-camera-e2e: swift-bridge
    cargo test -p aleph-desktop-macos --test camera_e2e -- --ignored --nocapture

# End-to-end: audio device listing + recording via the Swift helper. Requires microphone permission.
test-audio-e2e: swift-bridge
    cargo test -p aleph-desktop-macos --test audio_e2e -- --ignored --nocapture
