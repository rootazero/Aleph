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
    cd crates/desktop-macos/bridge && swift build -c release
    @echo "✓ Swift bridge: crates/desktop-macos/bridge/.build/release/AlephBridge"

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
test-all: test test-desktop-all test-proptest

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

# Create a new release: bump version, generate changelog, commit, push, trigger workflow
# Usage: just release 0.3.0
release version:
    #!/usr/bin/env bash
    set -euo pipefail
    VERSION="{{version}}"

    # Get previous version tag
    PREV_TAG=$(git tag --sort=-v:refname | head -1 || echo "")

    # Generate changelog from git log
    echo "Generating changelog for v${VERSION}..."
    if [ -n "$PREV_TAG" ]; then
        COMMITS=$(git log "${PREV_TAG}..HEAD" --pretty=format:"- %s" --no-merges | grep -v "Co-Authored-By" || true)
        RANGE="${PREV_TAG}..HEAD"
    else
        COMMITS=$(git log --pretty=format:"- %s" --no-merges -50 | grep -v "Co-Authored-By" || true)
        RANGE="(all)"
    fi

    # Categorize commits
    FEATURES=$(echo "$COMMITS" | grep -iE "^- (feat|add|new|desktop|core:.*add|core:.*implement)" || true)
    FIXES=$(echo "$COMMITS" | grep -iE "^- (fix|bugfix|hotfix)" || true)
    BUILD=$(echo "$COMMITS" | grep -iE "^- (build|ci|release|docs)" || true)
    REFACTOR=$(echo "$COMMITS" | grep -iE "^- (refactor|clean|remove|phase)" || true)

    # Build changelog entry
    ENTRY="## [${VERSION}] - $(date +%Y-%m-%d)\n"
    if [ -n "$FEATURES" ]; then
        ENTRY="${ENTRY}\n### Added\n${FEATURES}\n"
    fi
    if [ -n "$FIXES" ]; then
        ENTRY="${ENTRY}\n### Fixed\n${FIXES}\n"
    fi
    if [ -n "$REFACTOR" ]; then
        ENTRY="${ENTRY}\n### Changed\n${REFACTOR}\n"
    fi
    if [ -n "$BUILD" ]; then
        ENTRY="${ENTRY}\n### Build\n${BUILD}\n"
    fi

    # Prepend to CHANGELOG.md (after the header)
    if [ -f CHANGELOG.md ]; then
        # Insert after "## [Unreleased]" line
        perl -i -pe "s/^## \[Unreleased\].*/## [Unreleased]\n\n$(echo -e "$ENTRY" | sed 's/[&/\]/\\&/g' | tr '\n' '\a' | sed 's/\a/\\n/g')/" CHANGELOG.md 2>/dev/null || {
            # Fallback: just prepend after header
            HEADER=$(head -7 CHANGELOG.md)
            BODY=$(tail -n +8 CHANGELOG.md)
            echo "$HEADER" > CHANGELOG.md
            echo "" >> CHANGELOG.md
            echo -e "$ENTRY" >> CHANGELOG.md
            echo "$BODY" >> CHANGELOG.md
        }
    fi

    # Update VERSION file
    echo "$VERSION" > VERSION

    # Stage, commit, push
    git add VERSION CHANGELOG.md
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
