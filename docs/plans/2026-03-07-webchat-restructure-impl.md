# WebChat Restructure Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Reorganize UI projects: move webchat to `apps/webchat/`, move control_plane to `apps/panel/`, delete `apps/dashboard/`, update all references.

**Architecture:** Three `git mv` operations, then update Cargo workspace members, internal path references, server auto-discovery paths, justfile variables, build script paths, crate naming, and documentation.

**Tech Stack:** git, Cargo workspace, justfile, Trunk, wasm-bindgen

---

### Task 1: Move webchat to apps/webchat/

**Files:**
- Move: `ui/webchat/` → `apps/webchat/`
- Delete: `ui/` (after move, should be empty)

**Step 1: Move the directory**

```bash
cd /Users/zouguojun/Workspace/Aleph
git mv ui/webchat apps/webchat
```

**Step 2: Remove empty ui/ directory**

```bash
rmdir ui/  # Only works if empty; if not, check what's left
```

**Step 3: Verify webchat files are in new location**

```bash
ls apps/webchat/src/App.tsx apps/webchat/package.json
```

Expected: both files exist.

**Step 4: Commit**

```bash
git add -A
git commit -m "restructure: move ui/webchat to apps/webchat"
```

---

### Task 2: Move control_plane to apps/panel/

**Files:**
- Move: `core/ui/control_plane/` → `apps/panel/`
- Delete: `core/ui/` (after move, should be empty)

**Step 1: Move the directory**

```bash
cd /Users/zouguojun/Workspace/Aleph
git mv core/ui/control_plane apps/panel
```

**Step 2: Remove empty core/ui/ directory**

```bash
rmdir core/ui/
```

**Step 3: Verify panel files are in new location**

```bash
ls apps/panel/Cargo.toml apps/panel/src/app.rs apps/panel/Trunk.toml
```

Expected: all files exist.

**Step 4: Commit**

```bash
git add -A
git commit -m "restructure: move core/ui/control_plane to apps/panel"
```

---

### Task 3: Delete apps/dashboard/

**Files:**
- Delete: `apps/dashboard/` (entire directory)

**Step 1: Remove the directory**

```bash
cd /Users/zouguojun/Workspace/Aleph
git rm -r apps/dashboard
```

**Step 2: Commit**

```bash
git commit -m "restructure: remove apps/dashboard (superseded by panel)"
```

---

### Task 4: Update Cargo workspace members

**Files:**
- Modify: `Cargo.toml` (root workspace)

**Step 1: Update workspace members**

In `Cargo.toml`, change the `[workspace] members` list:

```toml
# Old entries to remove/change:
#   "core/ui/control_plane",
#   "apps/dashboard",

# New entry:
#   "apps/panel",
```

The full members list should be:

```toml
members = [
    "core",
    "apps/panel",
    "crates/desktop",
    "crates/logging",
    "shared/protocol",
    "shared/ui_logic",
    "shared_ui_logic",
    "apps/cli",
    "apps/shared",
    "apps/desktop/src-tauri",
]
```

**Step 2: Verify workspace resolves**

```bash
cargo metadata --no-deps --format-version=1 | python3 -c "import sys,json; pkgs=[p['name'] for p in json.load(sys.stdin)['packages']]; print('\n'.join(sorted(pkgs)))"
```

Expected: `aleph-control-plane` (or `aleph-panel`) in the list, no `aleph-dashboard`.

**Step 3: Commit**

```bash
git add Cargo.toml
git commit -m "restructure: update workspace members for panel move"
```

---

### Task 5: Update Panel Cargo.toml (crate name + shared-ui-logic path)

**Files:**
- Modify: `apps/panel/Cargo.toml`

**Step 1: Update package name and lib name**

Change:
```toml
[package]
name = "aleph-control-plane"

[lib]
name = "aleph_dashboard"
```

To:
```toml
[package]
name = "aleph-panel"

[lib]
name = "aleph_panel"
```

**Step 2: Update shared-ui-logic path**

The old path was relative from `core/ui/control_plane/`:
```toml
shared-ui-logic = { path = "../../../shared/ui_logic", features = ["leptos", "wasm"] }
```

From `apps/panel/`, the new relative path is:
```toml
shared-ui-logic = { path = "../../shared/ui_logic", features = ["leptos", "wasm"] }
```

**Step 3: Update main.rs to use new crate name**

In `apps/panel/src/main.rs`, change:
```rust
use aleph_dashboard::app::*;
```
To:
```rust
use aleph_panel::app::*;
```

**Step 4: Verify it compiles**

```bash
cargo check -p aleph-panel
```

Expected: compiles without errors (or only WASM-target warnings).

Note: This may fail because aleph-panel is a `cdylib` WASM target. In that case, try:
```bash
cargo check -p aleph-panel --target wasm32-unknown-unknown
```

If the WASM target isn't installed, just verify the workspace resolves:
```bash
cargo metadata --no-deps 2>&1 | head -5
```

Expected: no "failed to resolve" errors.

**Step 5: Commit**

```bash
git add apps/panel/Cargo.toml apps/panel/src/main.rs
git commit -m "restructure: rename crate aleph-control-plane to aleph-panel"
```

---

### Task 6: Update core/build.rs paths

**Files:**
- Modify: `core/build.rs`

**Step 1: Update all control_plane paths to panel paths**

The build script references `ui/control_plane` (relative to `core/`). After the move, the panel is at `apps/panel/` (relative to workspace root), which is `../apps/panel` relative to `core/`.

Change all occurrences:

| Old (relative to `core/`) | New (relative to `core/`) |
|---|---|
| `ui/control_plane` | `../apps/panel` |
| `ui/control_plane/dist` | `../apps/panel/dist` |
| `ui/control_plane/src` | `../apps/panel/src` |
| `ui/control_plane/Cargo.toml` | `../apps/panel/Cargo.toml` |
| `ui/control_plane/index.html` | `../apps/panel/index.html` |

Also update the warning messages from "ControlPlane" to "Panel".

The full updated `core/build.rs`:

```rust
// Build script for Aleph Core
//
// When control-plane feature is enabled:
// - Watches dist/ so rust-embed re-embeds when WASM assets change
// - Falls back to trunk build if dist/ is missing (for `cargo run` without justfile)

fn main() {
    #[cfg(feature = "control-plane")]
    {
        use std::path::Path;
        use std::process::Command;

        let panel_dir = Path::new("../apps/panel");
        let dist_dir = panel_dir.join("dist");

        // Watch dist/ files so cargo recompiles when assets change (rust-embed)
        println!("cargo:rerun-if-changed=../apps/panel/dist");
        if dist_dir.exists() {
            if let Ok(entries) = std::fs::read_dir(&dist_dir) {
                for entry in entries.flatten() {
                    println!("cargo:rerun-if-changed={}", entry.path().display());
                }
            }
        }

        // Watch source for fallback trunk build trigger
        println!("cargo:rerun-if-changed=../apps/panel/src");
        println!("cargo:rerun-if-changed=../apps/panel/Cargo.toml");
        println!("cargo:rerun-if-changed=../apps/panel/index.html");

        if !panel_dir.exists() {
            println!("cargo:warning=Panel directory not found, skipping UI build");
            return;
        }

        // If dist/ already has files (built by `just wasm`), skip trunk
        if dist_dir.exists() && dist_dir.read_dir().map(|mut d| d.next().is_some()).unwrap_or(false) {
            println!("cargo:warning=Panel UI assets found in dist/, embedding into binary");
            return;
        }

        // Fallback: try trunk build for `cargo run --features control-plane` without justfile
        println!("cargo:warning=Building Panel UI via trunk...");

        match Command::new("trunk")
            .args(&["build", "--release"])
            .current_dir(panel_dir)
            .status()
        {
            Ok(status) if status.success() => {
                println!("cargo:warning=Panel UI built successfully");
            }
            Ok(_) => {
                println!("cargo:warning=Panel build failed. Server will run without UI.");
                println!("cargo:warning=Run `just wasm` first, or fix trunk issues.");
            }
            Err(e) => {
                println!("cargo:warning=Failed to execute trunk: {}. Server will run without UI.", e);
                println!("cargo:warning=Run `just wasm` first, or install trunk.");
            }
        }
    }
}
```

**Step 2: Verify core compiles**

```bash
cargo check -p alephcore
```

Expected: compiles (the `control-plane` feature is off by default, so build.rs code is inert).

**Step 3: Commit**

```bash
git add core/build.rs
git commit -m "restructure: update build.rs paths for panel move"
```

---

### Task 7: Update justfile

**Files:**
- Modify: `justfile`

**Step 1: Update variables and wasm recipe**

Change the variables at top:

```just
# Old:
wasm_dir        := "core/ui/control_plane"
wasm_dist       := "core/ui/control_plane/dist"

# New:
panel_dir       := "apps/panel"
panel_dist      := "apps/panel/dist"
```

Update the `wasm` recipe — replace all `{{wasm_dir}}` with `{{panel_dir}}`, `{{wasm_dist}}` with `{{panel_dist}}`, and update crate/binary names:

```just
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
```

Update the `clean` recipe:

```just
clean:
    cargo clean
    rm -rf {{panel_dist}}
    rm -rf {{macos_dir}}/build
    rm -rf {{macos_resources}}/{{server_bin}}
    @echo "✓ Cleaned"
```

Add webchat recipes after the `wasm` recipe:

```just
# Build WebChat UI (React)
webchat-build:
    cd apps/webchat && pnpm build

# Dev server for WebChat
webchat-dev:
    cd apps/webchat && pnpm dev
```

**Step 2: Verify justfile parses**

```bash
just --list
```

Expected: all recipes listed without parse errors.

**Step 3: Commit**

```bash
git add justfile
git commit -m "restructure: update justfile for panel/webchat paths"
```

---

### Task 8: Update serve_webchat auto-discovery paths

**Files:**
- Modify: `core/src/bin/aleph/commands/start/builder/handlers.rs` (lines 286-296)

**Step 1: Update the candidate paths**

Change the auto-discovery paths in `start_webchat_server`:

```rust
// Old:
let mut candidates = vec![
    PathBuf::from("ui/webchat/dist"),
    PathBuf::from("../ui/webchat/dist"),
];

// New:
let mut candidates = vec![
    PathBuf::from("apps/webchat/dist"),
    PathBuf::from("../apps/webchat/dist"),
];
```

Also update the comment:
```rust
// Old: // Try default locations: ./ui/webchat/dist or ../ui/webchat/dist or ~/.aleph/webchat
// New: // Try default locations: ./apps/webchat/dist or ../apps/webchat/dist or ~/.aleph/webchat
```

**Step 2: Verify core compiles**

```bash
cargo check -p alephcore
```

Expected: compiles without errors.

**Step 3: Commit**

```bash
git add core/src/bin/aleph/commands/start/builder/handlers.rs
git commit -m "restructure: update webchat auto-discovery paths"
```

---

### Task 9: Update rust-embed path in core (if exists)

**Files:**
- Search: `core/src/` for `rust-embed` or `#[folder =` referencing control_plane

**Step 1: Search for embed references**

```bash
cd /Users/zouguojun/Workspace/Aleph
grep -r "ui/control_plane\|folder.*control" core/src/ --include="*.rs" -l
```

If any files are found, update `ui/control_plane/dist/` to `../apps/panel/dist/` (or the appropriate relative path from `core/`).

If no files are found, skip to commit.

**Step 2: Commit (if changes made)**

```bash
git add core/src/
git commit -m "restructure: update rust-embed paths for panel"
```

---

### Task 10: Update documentation

**Files:**
- Modify: `docs/reference/SERVER_DEVELOPMENT.md`

**Step 1: Update all path references**

Search and replace in `docs/reference/SERVER_DEVELOPMENT.md`:

| Old | New |
|-----|-----|
| `core/ui/control_plane` | `apps/panel` |
| `aleph-dashboard` | `aleph-panel` |
| `aleph_dashboard` | `aleph_panel` |
| `aleph-control-plane` | `aleph-panel` |
| `ui/control_plane/dist` | `apps/panel/dist` |

**Step 2: Update any .rs file header comments**

Some files in panel have header comments with old paths like:
```rust
// core/ui/control_plane/src/views/chat/view.rs
```

Update them to:
```rust
// apps/panel/src/views/chat/view.rs
```

Search for these:
```bash
grep -r "core/ui/control_plane" apps/panel/src/ --include="*.rs" -l
```

Update each found file.

**Step 3: Commit**

```bash
git add docs/reference/SERVER_DEVELOPMENT.md apps/panel/src/
git commit -m "docs: update paths for panel restructure"
```

---

### Task 11: Final verification

**Step 1: Verify workspace compiles**

```bash
cargo check -p alephcore
```

Expected: compiles without errors.

**Step 2: Run core tests**

```bash
cargo test -p alephcore --lib 2>&1 | tail -5
```

Expected: tests pass (pre-existing failures OK).

**Step 3: Verify justfile**

```bash
just --list
```

Expected: all recipes listed.

**Step 4: Verify no stale references**

```bash
grep -r "ui/webchat\|apps/dashboard\|core/ui/control_plane\|aleph.dashboard\|aleph_dashboard" \
    --include="*.rs" --include="*.toml" --include="*.md" \
    --exclude-dir=target --exclude-dir=node_modules --exclude-dir=.git \
    /Users/zouguojun/Workspace/Aleph/ | grep -v "plans/" | head -20
```

Expected: no matches (ignore docs/plans/ which reference old paths in context).

**Step 5: Squash or keep commits (user choice)**

All 10 commits can be kept as-is for clear history, or squashed into one:

```bash
# Optional squash (only if user wants):
# git rebase -i HEAD~10
```
