# Aleph Build Pipeline
# Usage: just <recipe>    Run: just --list

set shell := ["bash", "-euo", "pipefail", "-c"]

# ─── Variables ───
release_dir     := "target/release"
debug_dir       := "target/debug"
panel_dir       := "interfaces/webchat"
panel_dist      := "interfaces/webchat/dist"
server_bin      := "aleph-server"
shell_dir       := "desktop/shell"

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

# ─── Desktop App (Tauri v2 native shell, with aleph-server bundled in) ───

# Stage the daemon (+ macOS Swift bridge) as Tauri externalBin inputs.
# `profile` is the target/ subdir to copy the daemon from (debug | release).
#
# Uses `install -m 0755` instead of `cp` so the staged file is guaranteed
# executable. cargo can drop +x on the friendly-name binary (target/<profile>/
# aleph-server) when its hardlink fast-path falls back to copy on certain
# filesystems — without this, the Tauri bundler ships a non-executable
# aleph-server in Aleph.app/Contents/MacOS/ and the shell fails to spawn
# the daemon with EACCES.
_stage-shell-binaries profile:
    #!/usr/bin/env bash
    set -euo pipefail
    triple="$(rustc -vV | sed -n 's/host: //p')"
    ext=""; [[ "$OSTYPE" == msys* || "$OSTYPE" == cygwin* ]] && ext=".exe"
    mkdir -p {{shell_dir}}/binaries
    install -m 0755 "target/{{profile}}/{{server_bin}}$ext" "{{shell_dir}}/binaries/aleph-server-$triple$ext"
    if [[ "$OSTYPE" == darwin* ]]; then
        install -m 0755 "{{release_dir}}/aleph-bridge" "{{shell_dir}}/binaries/AlephBridge-$triple"
    fi

# Create empty externalBin placeholders so `cargo check` / `clippy` of the
# shell crate pass without a full daemon build (tauri-build requires the
# externalBin files to exist). `touch` leaves any real staged binary intact.
_stage-shell-placeholders:
    #!/usr/bin/env bash
    set -euo pipefail
    triple="$(rustc -vV | sed -n 's/host: //p')"
    ext=""; [[ "$OSTYPE" == msys* || "$OSTYPE" == cygwin* ]] && ext=".exe"
    mkdir -p {{shell_dir}}/binaries
    touch "{{shell_dir}}/binaries/aleph-server-$triple$ext"
    if [[ "$OSTYPE" == darwin* ]]; then
        touch "{{shell_dir}}/binaries/AlephBridge-$triple"
    fi

# Run the desktop app in dev mode (rebuilds + stages the daemon first)
shell-dev: build-debug
    #!/usr/bin/env bash
    set -euo pipefail
    if [[ "$OSTYPE" == darwin* ]]; then just swift-bridge; fi
    just _stage-shell-binaries debug
    version="$(tr -d '[:space:]' < VERSION)"
    cd {{shell_dir}} && cargo tauri dev --config "{\"version\":\"$version\"}"

# Build the desktop app installers (.dmg/.msi/.deb/…), daemon bundled inside.
#
# CI=true is exported so tauri-bundler passes --skip-jenkins to bundle_dmg.sh,
# which skips the osascript "tell Finder" step. That AppleScript call requires
# a TCC Automation→Finder grant on the parent process and silently exits 64
# from non-Terminal contexts (Claude Code, CI runners, ssh sessions), leaving
# rw.PID.*.dmg orphans in bundle/macos/. Skipping it produces a working DMG
# without the fancy icon layout — same as what CI ships.
shell-build: build
    #!/usr/bin/env bash
    set -euo pipefail
    just _stage-shell-binaries release
    version="$(tr -d '[:space:]' < VERSION)"
    cd {{shell_dir}} && CI=true cargo tauri build --config "{\"version\":\"$version\"}"
    echo "✓ Installers: {{release_dir}}/bundle/"

# ─── Single Stage ───

# Build WASM Panel UI only
wasm:
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p {{panel_dist}}
    # 1. Tailwind CSS
    (cd {{panel_dir}} && npm run build:css)
    # 2. Compile Rust → WASM
    cargo build -p aleph-panel --target wasm32-unknown-unknown --profile wasm-release
    # 3. Generate JS bindings
    wasm-bindgen --target web --no-typescript \
        --out-dir {{panel_dist}} --out-name aleph_panel \
        target/wasm32-unknown-unknown/wasm-release/aleph_panel.wasm
    # 3.5 Shrink wasm (optional; -g keeps the name section for crash diagnostics)
    if command -v wasm-opt >/dev/null 2>&1; then
        wasm-opt -Oz -g {{panel_dist}}/aleph_panel_bg.wasm -o {{panel_dist}}/aleph_panel_bg.wasm
        echo "✓ wasm-opt applied"
    else
        echo "⚠ wasm-opt not found; skipping (cargo install wasm-opt / brew install binaryen)"
    fi
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

# Quick check: desktop shell compiles
check-shell: _stage-shell-placeholders
    cargo check -p aleph-desktop-shell

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

# Clippy on the desktop shell
clippy-shell: _stage-shell-placeholders
    cargo clippy -p aleph-desktop-shell -- -D warnings

# Clippy everything
clippy-all: clippy clippy-desktop clippy-desktop-macos clippy-shell

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

# Verify the three-platform desktop App builds on CI without publishing.
# Triggers aleph-app-release.yml in build-only mode (publish=off): builds the
# macOS / Windows / Linux apps + uploads artifacts; no tag, no GitHub Release.
# Runs against the current origin/main — push local commits first if needed.
verify-build:
    gh workflow run aleph-app-release.yml --field publish=false
    @echo ""
    @echo "✓ Build-only verification triggered (no tag, no Release)."
    @echo "  Monitor: gh run list --workflow aleph-app-release.yml --limit 1"

# Release a new version: bump VERSION, commit, push, trigger the app workflow.
# Runs the workflow in publish mode — builds the three-platform desktop apps
# and publishes a GitHub Release. CHANGELOG.md must be written by AI (Claude)
# BEFORE running this command.
# Usage: just release 26.5.21
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

    # Update VERSION file (single source of truth) AND mirror it into
    # [workspace.package].version so every workspace member (alephcore,
    # aleph-cli, aleph-tui, aleph-desktop-*, etc.) inherits the same
    # CalVer at compile time. ALEPH_VERSION (set by build.rs from VERSION)
    # remains authoritative for any code that reads env!("ALEPH_VERSION").
    echo "$VERSION" > VERSION
    sed -i '' -E "s/^version = \"[0-9.]+\"$/version = \"${VERSION}\"/" Cargo.toml

    # Stage, commit, push
    git add -f VERSION Cargo.toml CHANGELOG.md
    git commit -m "release: v${VERSION}"
    git push origin main

    # Delete old release if exists
    gh release delete "v${VERSION}" --yes 2>/dev/null || true
    git push origin ":refs/tags/v${VERSION}" 2>/dev/null || true

    # Trigger the app build + publish workflow
    gh workflow run aleph-app-release.yml --field publish=true

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

# End-to-end: speech recognition via the Swift helper. Requires Speech + Microphone TCC.
test-speech-e2e: swift-bridge
    cargo test -p aleph-desktop-macos --test speech_e2e -- --ignored --nocapture

# End-to-end: OCR via the Swift helper. Requires no TCC (image is supplied directly).
test-ocr-e2e: swift-bridge
    cargo test -p aleph-desktop-macos --test ocr_e2e -- --ignored --nocapture
