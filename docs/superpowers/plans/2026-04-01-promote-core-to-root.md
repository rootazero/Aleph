# Promote core/ to Project Root — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Eliminate the `core/` wrapper directory by promoting its contents to the project root, making `src/` the top-level source directory.

**Architecture:** Merge `Cargo.toml` into root `Cargo.toml` (workspace + package in one file). Move all `core/` contents (src/, tests/, benches/, build.rs, etc.) to root. Update path references in build.rs, CI, and documentation.

**Tech Stack:** Rust workspace, Cargo, GitHub Actions CI

**Spec:** `docs/superpowers/specs/2026-04-01-promote-core-to-root-design.md`

---

### Task 1: Move source directories with git mv

**Files:**
- Move: `src/` → `src/`
- Move: `build.rs` → `build.rs`
- Move: `benches/` → `benches/`
- Move: `core/examples/` → `examples/` (root already has `examples/`, merge)
- Move: `core/bindings/` → `bindings/`
- Move: `core/proptest-regressions/` → `proptest-regressions/`
- Move: `core/config.search.example.toml` → `config.search.example.toml`
- Move: `core/test.txt` → (discard, temp file)

> **IMPORTANT:** The root already has `examples/` and `tests/` directories. Check for file name collisions before moving. Use `git mv` so git tracks renames.

- [ ] **Step 1: Verify no file name collisions in examples/**

```bash
ls /Volumes/TBU/Workspace/Aleph/examples/
ls /Volumes/TBU/Workspace/Aleph/core/examples/
```

Expected: Different file names in each directory (root has demo apps, core has `.rs` example files).

- [ ] **Step 2: Move src/ to root**

```bash
cd /Volumes/TBU/Workspace/Aleph
git mv core/src src
```

Expected: `src/` now exists at root with all 70+ modules.

- [ ] **Step 3: Move build.rs to root**

```bash
git mv build.rs build.rs
```

- [ ] **Step 4: Move benches/ to root**

```bash
git mv core/benches benches
```

- [ ] **Step 5: Move core/examples/ contents into root examples/**

```bash
git mv core/examples/*.rs examples/
```

- [ ] **Step 6: Move remaining core/ items**

```bash
git mv core/bindings bindings
git mv core/proptest-regressions proptest-regressions
git mv core/config.search.example.toml config.search.example.toml
```

- [ ] **Step 7: Merge tests/ into root tests/**

```bash
# Move all files and directories from tests/ into root tests/
for item in tests/*; do
  git mv "$item" tests/
done
```

Expected: Root `tests/` now contains both the original Python/E2E tests and all Rust integration tests.

- [ ] **Step 8: Merge core/docs/plans/ into docs/plans/**

```bash
mkdir -p docs/plans
git mv core/docs/plans/*.md docs/plans/
```

- [ ] **Step 9: Merge core/.claude/homunculus/ into root .claude/**

```bash
git mv core/.claude/homunculus .claude/homunculus
```

- [ ] **Step 10: Remove leftover core/ directory**

```bash
# Remove any remaining empty dirs or temp files
rm -f core/test.txt
# git will auto-remove empty directories; verify core/ is gone
rm -rf core/
git add -A core/
```

- [ ] **Step 11: Commit**

```bash
git add -A
git commit -m "refactor: move core/ contents to project root (step 1/5)"
```

---

### Task 2: Merge Cargo.toml files

**Files:**
- Modify: `Cargo.toml` (root) — merge in package sections from `Cargo.toml`
- Delete: `Cargo.toml` (already moved/removed in Task 1)

- [ ] **Step 1: Read both Cargo.toml files for reference**

Read `Cargo.toml` (root) — currently workspace-only.
The `Cargo.toml` content is already captured in the spec. Key sections to merge:
- `[package]` with name, version, edition, rust-version
- `[lib]` with crate-type and name
- `[features]`
- `[dependencies]` (all entries, with path adjustments)
- `[target.'cfg(...)'.dependencies]` (3 platform sections)
- `[build-dependencies]`
- `[[bin]]`
- `[dev-dependencies]`
- `[[bench]]`
- `[[test]]`

- [ ] **Step 2: Edit root Cargo.toml — remove "core" from workspace members**

In `Cargo.toml`, remove the line `"core",` from `[workspace] members`.

- [ ] **Step 3: Append [package] section after [workspace] block**

Add after the `[workspace.dependencies]` section and before `[profile.release]`:

```toml
[package]
name = "alephcore"
version = "0.2.5"
edition = "2021"
rust-version = "1.92"

[lib]
crate-type = ["rlib", "cdylib"]
name = "alephcore"

[features]
default = []
loom = ["dep:loom"]
test-helpers = []
control-plane = []
```

- [ ] **Step 4: Add all [dependencies] with updated paths**

Add the full `[dependencies]` block from `Cargo.toml`. For every `path = "../..."` entry, change to `path = "..."` (remove the `../` prefix). The 7 path entries that change:

```toml
aleph-protocol = { path = "shared/protocol" }     # was ../shared/protocol
aleph-logging = { path = "crates/logging" }        # was ../crates/logging
aleph-desktop = { path = "crates/desktop" }        # was ../crates/desktop
aleph-client = { path = "shared/client" }          # was ../shared/client
```

All other dependencies (from crates.io) are copied verbatim.

- [ ] **Step 5: Add platform-specific dependencies with updated paths**

```toml
[target.'cfg(target_os = "macos")'.dependencies]
aleph-desktop-macos = { path = "crates/desktop-macos" }   # was ../crates/desktop-macos

[target.'cfg(target_os = "linux")'.dependencies]
aleph-desktop-linux = { path = "crates/desktop-linux" }    # was ../crates/desktop-linux

[target.'cfg(target_os = "windows")'.dependencies]
aleph-desktop-windows = { path = "crates/desktop-windows" } # was ../crates/desktop-windows
```

- [ ] **Step 6: Add remaining sections**

```toml
[build-dependencies]
# No build dependencies - using Gateway WebSocket architecture

[[bin]]
name = "aleph-server"
path = "src/bin/aleph-server/main.rs"

[dev-dependencies]
criterion = { version = "0.5", features = ["html_reports", "async_tokio"] }
cucumber = { version = "0.21", features = ["macros"] }
tempfile = "3.8"
filetime = "0.2"
serial_test = "3.0"
aleph-protocol = { path = "shared/protocol" }   # was ../shared/protocol
proptest = "1.4"
http-body-util = "0.1"
wiremock = "0.6"

[[bench]]
name = "memory_benchmarks_simple"
harness = false

[[bench]]
name = "approval_performance"
harness = false

[[test]]
name = "cron_probe"
required-features = ["test-helpers"]

[[test]]
name = "cucumber"
harness = false
```

- [ ] **Step 7: Verify the merged Cargo.toml is valid**

```bash
cargo check -p alephcore 2>&1 | head -20
```

Expected: No TOML parse errors. May see compilation errors if paths aren't fully updated yet — that's fine, we'll fix in next steps.

- [ ] **Step 8: Commit**

```bash
git add Cargo.toml
git commit -m "refactor: merge Cargo.toml into root workspace Cargo.toml (step 2/5)"
```

---

### Task 3: Update build.rs paths

**Files:**
- Modify: `build.rs` (root, moved from core/)

- [ ] **Step 1: Update VERSION path**

In `build.rs`, replace:
```rust
println!("cargo:rerun-if-changed=../VERSION");
```
with:
```rust
println!("cargo:rerun-if-changed=VERSION");
```

And replace:
```rust
if let Ok(version) = std::fs::read_to_string("../VERSION") {
```
with:
```rust
if let Ok(version) = std::fs::read_to_string("VERSION") {
```

- [ ] **Step 2: Update control-plane paths**

Replace all `../interfaces/webchat` with `interfaces/webchat`:

```rust
let control_plane_dir = Path::new("interfaces/webchat");
// ...
println!("cargo:rerun-if-changed=interfaces/webchat/dist");
// ...
println!("cargo:rerun-if-changed=interfaces/webchat/src");
println!("cargo:rerun-if-changed=interfaces/webchat/Cargo.toml");
println!("cargo:rerun-if-changed=interfaces/webchat/index.html");
```

There are 6 occurrences of `../` in build.rs to replace (all become paths without `../`).

- [ ] **Step 3: Verify build.rs compiles**

```bash
cargo check -p alephcore 2>&1 | head -5
```

Expected: No errors related to build.rs. If compilation succeeds, `ALEPH_VERSION` is being read correctly.

- [ ] **Step 4: Commit**

```bash
git add build.rs
git commit -m "refactor: update build.rs paths after core/ promotion (step 3/5)"
```

---

### Task 4: Update CI workflow

**Files:**
- Modify: `.github/workflows/aleph-core-ci.yml`

- [ ] **Step 1: Update push path filters**

Replace:
```yaml
on:
  push:
    paths:
      - 'core/**'
      - 'crates/**'
      - 'Cargo.toml'
      - '.github/workflows/aleph-core-ci.yml'
```
with:
```yaml
on:
  push:
    paths:
      - 'src/**'
      - 'build.rs'
      - 'tests/**'
      - 'benches/**'
      - 'crates/**'
      - 'shared/**'
      - 'Cargo.toml'
      - 'Cargo.lock'
      - '.github/workflows/aleph-core-ci.yml'
```

- [ ] **Step 2: Update pull_request path filters**

Replace:
```yaml
  pull_request:
    paths:
      - 'core/**'
      - 'crates/**'
      - 'Cargo.toml'
```
with:
```yaml
  pull_request:
    paths:
      - 'src/**'
      - 'build.rs'
      - 'tests/**'
      - 'benches/**'
      - 'crates/**'
      - 'shared/**'
      - 'Cargo.toml'
      - 'Cargo.lock'
```

- [ ] **Step 3: Remove rust-cache workspace overrides**

Remove `workspaces: core` from all 3 `Swatinem/rust-cache@v2` blocks (lines 28, 52, 77). The cache defaults to the root workspace, which is now correct.

Replace each:
```yaml
      - uses: Swatinem/rust-cache@v2
        with:
          workspaces: core
```
with:
```yaml
      - uses: Swatinem/rust-cache@v2
```

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/aleph-core-ci.yml
git commit -m "ci: update path filters and cache config after core/ promotion (step 4/5)"
```

---

### Task 5: Update documentation references

**Files:**
- Modify: `CLAUDE.md`
- Modify: `README.md`
- Modify: All files in `docs/reference/` containing `src/`
- Modify: All other files containing `src/`, `Cargo.toml`, `tests/`, `build.rs`

- [ ] **Step 1: Find all files with stale core/ references**

```bash
grep -r --include="*.md" -l 'src/\|core/Cargo\.toml\|tests/\|core/build\.rs\|benches/' /Volumes/TBU/Workspace/Aleph/docs/ /Volumes/TBU/Workspace/Aleph/CLAUDE.md /Volumes/TBU/Workspace/Aleph/README.md
```

Expected: List of 15-30 files needing updates.

- [ ] **Step 2: Batch replace core/ references in docs**

Use `sed` for bulk replacement across all markdown files:

```bash
cd /Volumes/TBU/Workspace/Aleph

# Replace src/ → src/
find docs/ -name "*.md" -exec sed -i '' 's|src/|src/|g' {} +

# Replace Cargo.toml → Cargo.toml (only standalone references)
find docs/ -name "*.md" -exec sed -i '' 's|core/Cargo\.toml|Cargo.toml|g' {} +

# Replace tests/ → tests/
find docs/ -name "*.md" -exec sed -i '' 's|tests/|tests/|g' {} +

# Replace build.rs → build.rs
find docs/ -name "*.md" -exec sed -i '' 's|core/build\.rs|build.rs|g' {} +

# Replace benches/ → benches/
find docs/ -name "*.md" -exec sed -i '' 's|benches/|benches/|g' {} +
```

- [ ] **Step 3: Update CLAUDE.md**

In `CLAUDE.md`:
- Redline R1: replace `core/src` with `src` (if present)
- Redline R3: update `core` reference to clarify it means root-level package
- Build commands table: `cargo check -p alephcore` stays unchanged (uses package name)
- Any `core/src` path references → `src`

```bash
sed -i '' 's|core/src|src|g' CLAUDE.md
```

- [ ] **Step 4: Update README.md project structure**

Manually update the project structure diagram in README.md to show `src/` at root level instead of `src/`. The exact lines will vary — read the file and update the tree diagram.

- [ ] **Step 5: Verify no stale references remain**

```bash
grep -r --include="*.md" 'src/' docs/ CLAUDE.md README.md
grep -r --include="*.md" 'core/Cargo\.toml' docs/ CLAUDE.md README.md
grep -r --include="*.md" 'tests/' docs/ CLAUDE.md README.md
```

Expected: No output (all references updated).

- [ ] **Step 6: Commit**

```bash
git add -A docs/ CLAUDE.md README.md
git commit -m "docs: update all core/ path references after promotion to root (step 5/5)"
```

---

### Task 6: Verify everything works

**Files:** None (verification only)

- [ ] **Step 1: Verify cargo check passes**

```bash
cargo check -p alephcore
```

Expected: Compilation succeeds with no errors.

- [ ] **Step 2: Verify unit tests pass**

```bash
cargo test -p alephcore --lib 2>&1 | tail -5
```

Expected: All tests pass.

- [ ] **Step 3: Verify binary builds**

```bash
cargo build --bin aleph-server 2>&1 | tail -3
```

Expected: Binary builds successfully.

- [ ] **Step 4: Verify no stale path references in Cargo.toml**

```bash
grep '"\.\.\/' Cargo.toml
```

Expected: No output (no leftover `../` paths).

- [ ] **Step 5: Verify core/ directory is fully removed**

```bash
ls -la core/ 2>&1
```

Expected: `No such file or directory`

- [ ] **Step 6: Verify cargo doc works (cdylib + workspace root check)**

```bash
cargo doc -p alephcore --no-deps 2>&1 | tail -5
```

Expected: Documentation generates without errors.

- [ ] **Step 7: Run clippy**

```bash
cargo clippy -p alephcore -- -D warnings 2>&1 | tail -5
```

Expected: No warnings.
