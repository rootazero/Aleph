# Rename crates/ to desktop/ — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rename `crates/` to `desktop/` with cleaner internal naming, and move `logging` to `shared/`.

**Architecture:** Use `git mv` to relocate directories. Update Cargo.toml path references (root + internal). Update CI path filters and documentation.

**Tech Stack:** Rust workspace, Cargo, GitHub Actions CI

**Spec:** `docs/superpowers/specs/2026-04-01-rename-crates-to-desktop-design.md`

---

### Task 1: Move directories with git mv

**Files:**
- Move: `crates/desktop/` → `desktop/shared/`
- Move: `crates/desktop-macos/` → `desktop/macos/`
- Move: `crates/desktop-linux/` → `desktop/linux/`
- Move: `crates/desktop-windows/` → `desktop/windows/`
- Move: `crates/logging/` → `shared/logging/`

- [ ] **Step 1: Create desktop/ parent directory and move shared crate**

```bash
cd /Volumes/TBU/Workspace/Aleph
mkdir desktop
git mv crates/desktop desktop/shared
```

- [ ] **Step 2: Move platform crates**

```bash
git mv crates/desktop-macos desktop/macos
git mv crates/desktop-linux desktop/linux
git mv crates/desktop-windows desktop/windows
```

- [ ] **Step 3: Move logging to shared/**

```bash
git mv crates/logging shared/logging
```

- [ ] **Step 4: Remove empty crates/ directory**

```bash
rm -rf crates/
```

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "refactor: rename crates/ to desktop/, move logging to shared/ (step 1/4)"
```

---

### Task 2: Update Cargo.toml path references

**Files:**
- Modify: `Cargo.toml` (root) — workspace members + dependencies
- Modify: `desktop/macos/Cargo.toml` — internal dependency path
- Modify: `desktop/linux/Cargo.toml` — internal dependency path
- Modify: `desktop/windows/Cargo.toml` — internal dependency path
- Modify: `interfaces/cli/Cargo.toml` — logging path
- Modify: `interfaces/tui/Cargo.toml` — logging path

- [ ] **Step 1: Update root Cargo.toml workspace members**

Replace in `Cargo.toml`:
```toml
# Before                          # After
"crates/desktop",          →      "desktop/shared",
"crates/desktop-macos",    →      "desktop/macos",
"crates/desktop-linux",    →      "desktop/linux",
"crates/desktop-windows",  →      "desktop/windows",
"crates/logging",          →      "shared/logging",
```

- [ ] **Step 2: Update root Cargo.toml [dependencies] paths**

Replace in `Cargo.toml`:
```toml
aleph-logging = { path = "shared/logging" }      # was crates/logging
aleph-desktop = { path = "desktop/shared" }       # was crates/desktop
```

- [ ] **Step 3: Update root Cargo.toml [target.cfg] dependency paths**

Replace in `Cargo.toml`:
```toml
aleph-desktop-macos = { path = "desktop/macos" }     # was crates/desktop-macos
aleph-desktop-linux = { path = "desktop/linux" }      # was crates/desktop-linux
aleph-desktop-windows = { path = "desktop/windows" }  # was crates/desktop-windows
```

- [ ] **Step 4: Update desktop platform crates internal dependency**

In `desktop/macos/Cargo.toml`, `desktop/linux/Cargo.toml`, and `desktop/windows/Cargo.toml`, replace:
```toml
# Before
aleph-desktop = { path = "../desktop" }
# After
aleph-desktop = { path = "../shared" }
```

- [ ] **Step 5: Update interfaces/cli and interfaces/tui logging path**

In `interfaces/cli/Cargo.toml`, replace:
```toml
# Before
aleph-logging = { path = "../../crates/logging" }
# After
aleph-logging = { path = "../../shared/logging" }
```

In `interfaces/tui/Cargo.toml`, same replacement:
```toml
aleph-logging = { path = "../../shared/logging" }
```

- [ ] **Step 6: Verify Cargo.toml is valid**

```bash
cargo check -p aleph-desktop 2>&1 | tail -5
```

Expected: Compilation succeeds (or only unrelated warnings).

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml desktop/macos/Cargo.toml desktop/linux/Cargo.toml desktop/windows/Cargo.toml interfaces/cli/Cargo.toml interfaces/tui/Cargo.toml
git commit -m "refactor: update all Cargo.toml paths for crates/ → desktop/ rename (step 2/4)"
```

---

### Task 3: Update CI workflow and documentation

**Files:**
- Modify: `.github/workflows/aleph-core-ci.yml` — path filters
- Modify: docs and markdown files referencing `crates/`

- [ ] **Step 1: Update CI path filters**

In `.github/workflows/aleph-core-ci.yml`, replace both occurrences of:
```yaml
- 'crates/**'
```
with:
```yaml
- 'desktop/**'
```

- [ ] **Step 2: Batch replace crates/ references in docs**

```bash
cd /Volumes/TBU/Workspace/Aleph

# Replace crates/desktop-macos → desktop/macos (do longer patterns first)
find docs/ -name "*.md" -exec sed -i '' 's|crates/desktop-macos|desktop/macos|g' {} +
find docs/ -name "*.md" -exec sed -i '' 's|crates/desktop-linux|desktop/linux|g' {} +
find docs/ -name "*.md" -exec sed -i '' 's|crates/desktop-windows|desktop/windows|g' {} +

# Replace crates/desktop/ → desktop/shared/ and crates/desktop → desktop/shared
find docs/ -name "*.md" -exec sed -i '' 's|crates/desktop/|desktop/shared/|g' {} +
find docs/ -name "*.md" -exec sed -i '' 's|crates/desktop|desktop/shared|g' {} +

# Replace crates/logging → shared/logging
find docs/ -name "*.md" -exec sed -i '' 's|crates/logging|shared/logging|g' {} +

# Same for CLAUDE.md and README.md
sed -i '' 's|crates/desktop-macos|desktop/macos|g' CLAUDE.md README.md
sed -i '' 's|crates/desktop-linux|desktop/linux|g' CLAUDE.md README.md
sed -i '' 's|crates/desktop-windows|desktop/windows|g' CLAUDE.md README.md
sed -i '' 's|crates/desktop/|desktop/shared/|g' CLAUDE.md README.md
sed -i '' 's|crates/desktop|desktop/shared|g' CLAUDE.md README.md
sed -i '' 's|crates/logging|shared/logging|g' CLAUDE.md README.md
```

- [ ] **Step 3: Verify no stale references**

```bash
grep -r --include="*.md" 'crates/desktop\|crates/logging' docs/ CLAUDE.md README.md
grep 'crates/' Cargo.toml
```

Expected: No output.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "docs+ci: update all crates/ references after rename to desktop/ (step 3/4)"
```

---

### Task 4: Verify everything works

**Files:** None (verification only)

- [ ] **Step 1: cargo check full workspace**

```bash
cargo check -p alephcore 2>&1 | tail -5
```

Expected: Compilation succeeds.

- [ ] **Step 2: Check desktop crate compiles**

```bash
cargo check -p aleph-desktop 2>&1 | tail -3
```

Expected: Succeeds.

- [ ] **Step 3: Check platform crate compiles (macOS)**

```bash
cargo check -p aleph-desktop-macos 2>&1 | tail -3
```

Expected: Succeeds.

- [ ] **Step 4: Verify crates/ directory is gone**

```bash
ls crates/ 2>&1
```

Expected: `No such file or directory`

- [ ] **Step 5: Verify no stale paths in any Cargo.toml**

```bash
grep -r 'crates/' Cargo.toml desktop/*/Cargo.toml shared/*/Cargo.toml interfaces/*/Cargo.toml
```

Expected: No output.

- [ ] **Step 6: Run unit tests**

```bash
cargo test -p aleph-desktop --lib 2>&1 | tail -5
```

Expected: Tests pass.
