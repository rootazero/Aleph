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

# Build Swift bridge (macOS only; no-op on Windows/Linux so `build` stays cross-platform)
swift-bridge:
    #!/usr/bin/env bash
    set -euo pipefail
    if [[ "$OSTYPE" != darwin* ]]; then
        echo "✓ Swift bridge: skipped (non-macOS)"
        exit 0
    fi
    # --product: the package also holds the test-only AlephFixture (an AppKit app),
    # and the shipped build has no business compiling it.
    #
    # The `cd` MUST stay inside the subshell. Every line below is repo-relative
    # (`{{release_dir}}`) or built from `$PWD`, so letting it leak wrote both
    # links to `desktop/macos/bridge/target/release/` and pointed them at
    # `…/bridge/desktop/macos/bridge/.build/…` — a doubled path, in a `target/`
    # dir nothing reads. The bridge built fine and `_stage-shell-binaries` then
    # failed on `install: target/release/aleph-bridge: No such file or
    # directory`, so the error named the consumer and never the cause.
    bridge=".build/release/AlephBridge"
    ( cd desktop/macos/bridge && swift build -c release --product AlephBridge )
    mkdir -p {{release_dir}} {{debug_dir}}
    ln -sf "$PWD/desktop/macos/bridge/$bridge" {{release_dir}}/aleph-bridge
    ln -sf "$PWD/desktop/macos/bridge/$bridge" {{debug_dir}}/aleph-bridge
    # Fail here rather than three recipes later: a dangling symlink resolves to
    # "No such file" at the `install` in `_stage-shell-binaries`, which reads
    # like the bridge was never built.
    test -x "{{release_dir}}/aleph-bridge" \
        || { echo "✗ Swift bridge: {{release_dir}}/aleph-bridge is missing or dangling"; exit 1; }
    echo "✓ Swift bridge: desktop/macos/bridge/$bridge"

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
    root="$PWD"
    (cd {{shell_dir}} && CI=true cargo tauri build --config "{\"version\":\"$version\"}")
    # macOS: Tauri signs the bundled aleph-server with the linker's per-build
    # ad-hoc identifier and ignores its embedded Info.plist. Re-sign so codesign
    # adopts the STABLE CFBundleIdentifier (ai.aleph.server), then re-seal the
    # outer app. The daemon detaches (--daemon: double-fork + setsid) so it is
    # its own macOS Local Network Privacy subject; a stable identity is what
    # lets its LAN-host access (self-hosted SearXNG / Firecrawl, …) be granted
    # once and persist across rebuilds instead of silently blocked.
    # See src/bin/aleph-server/Info.plist + build.rs.
    if [[ "$OSTYPE" == darwin* ]]; then
        app="$root/{{release_dir}}/bundle/macos/Aleph.app"
        codesign -s - -f "$app/Contents/MacOS/aleph-server"
        codesign -s - -f "$app"
    fi
    echo "✓ Installers: {{release_dir}}/bundle/"

# Build the panel-only desktop shell (no embedded aleph-server daemon).
#
# A distinct bundle identifier + productName ("Aleph Panel") so it installs
# alongside the full app. `--no-default-features` (forwarded to cargo after
# `--`) drops the embedded-core code path; `tauri.lite.conf.json` clears
# externalBin so no daemon is bundled. The splash frontend is self-contained
# HTML, so this needs no WASM/Swift build.
shell-build-lite:
    #!/usr/bin/env bash
    set -euo pipefail
    version="$(tr -d '[:space:]' < VERSION)"
    cd {{shell_dir}} && CI=true cargo tauri build \
        --config tauri.lite.conf.json --config "{\"version\":\"$version\"}" \
        -- --no-default-features
    echo "✓ Lite installers: {{release_dir}}/bundle/"

# Run the panel-only shell in dev mode (no embedded daemon).
shell-dev-lite:
    #!/usr/bin/env bash
    set -euo pipefail
    version="$(tr -d '[:space:]' < VERSION)"
    cd {{shell_dir}} && cargo tauri dev \
        --config tauri.lite.conf.json --config "{\"version\":\"$version\"}" \
        -- --no-default-features

# ─── Single Stage ───

# Build WASM Panel UI only
wasm:
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p {{panel_dist}}
    # 1. Tailwind CSS
    (cd {{panel_dir}} && npm run build:css)
    # 2. Compile Rust → WASM (lib only: the cdylib is the shipped artifact and
    #    the vestigial src/main.rs bin breaks under fat LTO)
    cargo build -p aleph-panel --lib --target wasm32-unknown-unknown --profile wasm-release
    # Resolve cargo's real target dir instead of hardcoding `target/`. A literal
    # relative path breaks in git worktrees: .cargo/config.toml pins an absolute
    # shared target-dir, so `cargo build` writes there while a cwd-relative
    # `target/` points at a nonexistent dir inside the worktree. `cargo metadata`
    # honors target-dir overrides and CARGO_TARGET_DIR, so it's correct everywhere.
    target_dir=$(cargo metadata --format-version 1 --no-deps \
        | node -e 'let s="";process.stdin.on("data",c=>s+=c).on("end",()=>process.stdout.write(JSON.parse(s).target_directory))')
    # 3. Generate JS bindings
    wasm-bindgen --target web --no-typescript \
        --out-dir {{panel_dist}} --out-name aleph_panel \
        "$target_dir/wasm32-unknown-unknown/wasm-release/aleph_panel.wasm"
    # 3.5 Shrink wasm AND fence its feature set.
    #
    # This step is REQUIRED, not an optional optimisation, and it validates
    # the module in TWO separate wasm-opt invocations, not one.
    #
    # Binaryen auto-detects a module's required feature set from the
    # `target_features` custom section LLVM embeds, and trusts it
    # unconditionally at PARSE time -- before any pass, including
    # --strip-target-features itself, gets a chance to run. So a single
    # `wasm-opt --strip-target-features -mvp --enable-... in -o out` does
    # NOT fence anything: verified against wasm-opt 130, it silently
    # accepted a module built with `-C target-feature=+simd128` with no
    # --enable-simd anywhere on the command line. The strip has to be its
    # own invocation, so the SECOND invocation parses a file that never had
    # the auto-detecting section to begin with.
    #
    #   Pass 1 (strip): remove the target_features section that would
    #   otherwise silently allow whatever the compiler happened to emit.
    #   Pass 2 (fence + shrink): -mvp starts validation from an EMPTY
    #   feature set, so only the explicit --enable-* flags built from
    #   interfaces/webchat/webview-baseline.json's "wasm_features" key are
    #   allowed. A feature the module actually uses that isn't on that list
    #   makes wasm-opt's own validator reject the module and name the
    #   feature in its error output -- a real validation failure, not a
    #   report we'd have to interpret ourselves.
    #
    # wasm_features is the single declaration of the floor -- do not
    # duplicate it here or anywhere else. If you're adding an entry,
    # adjudicate it against Safari 16.4 first (see the notes already in that
    # file) and add it there. If a toolchain bump makes this fence go red,
    # `wasm-opt --print-features` on the pre-strip artifact (still intact at
    # {{panel_dist}}/aleph_panel_bg.wasm if pass 2 failed -- it only gets
    # overwritten on success) shows the module's real required set to
    # compare against the floor.
    #
    # What this DOES guarantee: the shipped module validates under MVP plus
    # exactly the features in wasm_features -- nothing else, regardless of
    # what the compiler's target_features section claims. What it does NOT
    # guarantee: that every feature on that list is actually still inside
    # the Safari/WebKitGTK floor declared elsewhere in the same file --
    # that's a human adjudication made once per entry, not something this
    # step re-checks.
    #
    # -g (both passes) keeps the name section for crash diagnostics --
    # dropped by pass 1 if omitted there, and pass 2 can't recover what
    # pass 1 already discarded. The intermediate stripped file is written
    # OUTSIDE dist/ (via mktemp): an aborted build (set -euo pipefail skips
    # the cleanup line below on failure) must not leave a stray file for
    # later dist/-scoped tasks to trip over.
    if ! command -v wasm-opt >/dev/null 2>&1; then
        echo "✗ wasm-opt (binaryen) is required." >&2
        echo "  It is not a size optimisation any more: it fences the WASM feature set" >&2
        echo "  against the declared WebView baseline. Install it with one of:" >&2
        echo "    cargo install wasm-opt" >&2
        echo "    brew install binaryen        # macOS" >&2
        echo "    apt install binaryen         # Debian/Ubuntu" >&2
        echo "    winget install WebAssembly.binaryen   # Windows" >&2
        exit 1
    fi
    wasm_enable_flags=$(node -e '
        const baseline = JSON.parse(require("fs").readFileSync("interfaces/webchat/webview-baseline.json", "utf8"));
        process.stdout.write(baseline.wasm_features.map(([flag]) => flag).join(" "));
    ')
    wasm_opt_tmp=$(mktemp)
    wasm-opt --strip-target-features -g \
        {{panel_dist}}/aleph_panel_bg.wasm -o "$wasm_opt_tmp"
    if ! wasm-opt -mvp -Oz -g $wasm_enable_flags \
        "$wasm_opt_tmp" -o {{panel_dist}}/aleph_panel_bg.wasm; then
        echo "✗ wasm module requires a WASM feature outside the declared WebView floor" >&2
        echo "  (see the validator error above for which one)." >&2
        echo "  Floor: interfaces/webchat/webview-baseline.json (wasm_features)" >&2
        echo "  Re-derive the module's real required feature set with:" >&2
        echo "    wasm-opt --print-features {{panel_dist}}/aleph_panel_bg.wasm" >&2
        rm -f "$wasm_opt_tmp"
        exit 1
    fi
    rm -f "$wasm_opt_tmp"
    echo "✓ wasm-opt applied (feature set fenced)"
    # 4. Runtime index.html. Written in three parts so baseline-probe.js is
    #    inlined VERBATIM: it must run synchronously before the module script
    #    (module scripts are deferred), and it must be byte-identical to its
    #    source so scripts/check_webview_baseline.mjs edge C can pair them.
    cat > {{panel_dist}}/index.html << 'HTMLHEAD'
    <!DOCTYPE html>
    <html lang="en">
      <head>
        <meta charset="utf-8" />
        <meta name="viewport" content="width=device-width, initial-scale=1" />
        <title>Aleph Panel</title>
        <!-- Inline so the browser never issues its default GET /favicon.ico:
             nothing serves that path, so every page load logged a 404 that cost a
             QA round to chase. A data: URI is inside the Panel CSP
             (img-src 'self' data: https:) and needs no new dist asset. -->
        <link rel="icon" href="data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 32 32'%3E%3Crect width='32' height='32' rx='7' fill='%230d0d12'/%3E%3Ctext x='16' y='23' text-anchor='middle' font-family='Georgia,serif' font-size='21' fill='%23e8e6e1'%3E%E2%84%B5%3C/text%3E%3C/svg%3E" />
        <link rel="stylesheet" href="/tailwind.css" />
      </head>
      <body class="bg-surface text-text-primary">
        <noscript>This application requires JavaScript to run.</noscript>
        <script>
    HTMLHEAD
    cat {{panel_dir}}/baseline-probe.js >> {{panel_dist}}/index.html
    cat >> {{panel_dist}}/index.html << 'HTMLTAIL'
        </script>
        <script type="module">
          import init from '/aleph_panel.js';
          await init({ module_or_path: '/aleph_panel_bg.wasm' });
        </script>
      </body>
    </html>
    HTMLTAIL
    # 4.5 Baseline consistency (edges A-D).
    node scripts/check_webview_baseline.mjs
    # 5. Guard: the freshly-written js + wasm MUST be a matched pair. Catches a
    #    js-only rebuild (the v26.6.22 blank-panel bug) before it can be committed.
    node scripts/check_panel_dist.mjs {{panel_dist}}
    echo "✓ WASM: {{panel_dist}}/"

# Verify the committed panel dist is a matched js+wasm pair (no closure-trampoline
# drift). Run by `just wasm`, in CI on every dist change, and gating each release.
check-dist:
    node scripts/check_panel_dist.mjs {{panel_dist}}

# Verify the Panel's declared WebView baseline is consistent across every
# consumer. Run by `just wasm`, and in CI on any change under
# interfaces/webchat/ or desktop/shell/.
check-baseline:
    node scripts/check_webview_baseline.mjs

# ─── Testing ───

# Quick check: core compiles
#
# `--all-targets` is not tidiness. Without it cargo does not compile
# `#[cfg(test)]` code, so this recipe — the documented dev-loop entry point —
# stays green while the lib test target is broken. That is not hypothetical:
# four tests for a deliberately-removed struct sat on main doing exactly that,
# and nothing said so until the CI test job spent 25 minutes finding out.
# It also covers tests/ and benches/. Checking the lib test target takes
# ~11.5 GB in one rustc, so on a 16 GB box the default parallelism gets it
# OOM-killed; add `-j1` if your machine is that tight (CI pins it for exactly
# this reason).
check:
    cargo check -p alephcore --all-targets

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
# Serialized: the desktop tool is a process singleton (one physical desktop,
# one HELD_LOCKS table). Running these 117 tests in parallel races a
# desktop-specific global on teardown — a non-deterministic SIGTRAP/SIGABRT
# (~60%) that never reproduces single-threaded or inside the full lib run.
# Logic all passes; --test-threads=1 is the correct isolation for singleton
# tests and keeps `just test-all` green.
test-desktop-integration:
    cargo test -p alephcore --lib builtin_tools::desktop -- --test-threads=1

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
test-all: test test-desktop-all test-proptest check-phase5 check-wiring

# Phase 5 exit criterion 9 gate.
check-phase5:
    ./scripts/check-phase5-exit.sh

# Wiring parity guards: catch "severed wire" drift where both ends exist but the
# connection is missing. Each is a grep-level diff against a triaged baseline —
# green now, fails only on a NEW severance. See the 2026-07-15 wire audit.
check-wiring: check-rpc-wiring check-tool-wiring check-config-wiring

# RPC: no Panel rpc_call / rate-lane classifier may name a method with no handler
# (→ METHOD_NOT_FOUND / phantom rate-lane guard).
check-rpc-wiring:
    python3 scripts/rpc_wiring_audit.py

# Tools: every AlephTool NAME defined must have a dispatch arm (else the LLM can
# never call it).
check-tool-wiring:
    python3 scripts/tool_wiring_audit.py

# Config: every [policies.*] section type must have a core-side consumer (else the
# knob is an inert R10 corpse).
check-config-wiring:
    python3 scripts/config_wiring_audit.py

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

# Verify the full release matrix builds on CI without publishing.
# Triggers aleph-app-release.yml in build-only mode (publish=off): builds all
# three deliverables across macOS / Windows / Linux — the full desktop App
# (aleph-server bundled), the Aleph Panel lite shell (no daemon), and the
# standalone aleph-server binary — and uploads artifacts; no tag, no Release.
# Runs against the current origin/main — push local commits first if needed.
verify-build:
    git submodule update --init --recursive
    gh workflow run aleph-app-release.yml --field publish=false
    @echo ""
    @echo "✓ Build-only verification triggered (no tag, no Release)."
    @echo "  Monitor: gh run list --workflow aleph-app-release.yml --limit 1"

# Release a new version: bump VERSION, commit, push, trigger the app workflow.
# Runs the workflow in publish mode — builds and publishes all three
# deliverables (full desktop App with aleph-server bundled, Aleph Panel lite
# shell, standalone aleph-server binary + install.sh) across the three
# platforms in a single GitHub Release. CHANGELOG.md must be written by AI
# (Claude) BEFORE running this command.
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

    # Bump bundled skills/plugins submodules to their latest upstream main so
    # this release embeds the newest official content (the offline fallback).
    git submodule update --remote --recursive

    # Stage, commit, push (submodule pointer bump rides along, recorded in the release commit)
    git add -f VERSION Cargo.toml CHANGELOG.md
    git add skills plugins
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

# Build the test-only AppKit fixture app driven by the computer-use e2e suite.
# Never shipped: `swift-bridge` builds --product AlephBridge only, and nothing in
# AlephBridge references this.
swift-fixture:
    #!/usr/bin/env bash
    set -euo pipefail
    if [[ "$OSTYPE" != darwin* ]]; then
        echo "✓ Swift fixture: skipped (non-macOS)"
        exit 0
    fi
    cd desktop/macos/bridge && swift build -c release --product AlephFixture
    echo "✓ Swift fixture: desktop/macos/bridge/.build/release/AlephFixture"

# End-to-end: the closed computer-use loop — drive the real bridge (real AX calls,
# real CGEvents) at the real AlephFixture app, then assert against the FIXTURE's
# own report of what happened to it.
#
# Requires Accessibility (TCC) for the helper. The Tier B tests additionally need a
# real logged-in GUI session: they post CGEvents at an on-screen window, and they
# sample the physical cursor — so do not touch the mouse while they run.
# --test-threads=1: the tests drive one shared desktop; two fixtures racing for
# focus and the cursor is not a thing a green can survive.
test-computer-use-e2e: swift-bridge swift-fixture
    cargo test -p aleph-desktop-macos --test computer_use_e2e -- --ignored --nocapture --test-threads=1

# Tier A only: the AX rail against an off-display fixture window. No GUI session
# interaction, no cursor sampling — still needs Accessibility.
test-computer-use-e2e-headless: swift-bridge swift-fixture
    cargo test -p aleph-desktop-macos --test computer_use_e2e tier_a -- --ignored --nocapture --test-threads=1

# ─── iOS Distribution ───

# Build + upload an iOS Panel distribution build to TestFlight (internal testing).
# Requires a paid Apple Developer membership + the ASC env vars
# (ALEPH_TEAM_ID / ASC_KEY_ID / ASC_ISSUER_ID / ASC_KEY_PATH).
# See mobile/ios/README.md → Distribution (TestFlight).
ios-testflight:
    cd mobile/ios && ./release-testflight.sh
