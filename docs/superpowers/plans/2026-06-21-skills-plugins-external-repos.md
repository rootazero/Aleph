# External Skills/Plugins Repos + Hub-Managed + Offline Fallback — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move the 37 official skills and 8 official plugins out of the main Aleph repo into two independent GitHub-synced repos, kept in the binary as a submodule-sourced offline fallback, obtained at first run via `git clone` (fallback to embedded), refreshed only on explicit trigger, and listed/installed through the Aleph Hub catalog like third-party extensions.

**Architecture:** Source of truth = `rootazero/Aleph-skills` + `rootazero/Aleph-plugins`. The main repo references them as git submodules at the unchanged `skills/`/`plugins/` paths, so `include_dir!` still embeds an offline-fallback snapshot. At runtime the existing extractor/manifest/reconcile machinery is reused; only its *source* changes — a freshly cloned isolated checkout when online, the embedded `Dir` otherwise. `~/.aleph/skills` and `~/.aleph/plugins` are never git working copies, so there is no merge-conflict surface.

**Tech Stack:** Rust (tokio, serde, git2/libgit2, include_dir 0.7), JSON-RPC gateway, `aleph-client` CLI, git submodules, GitHub Actions (release workflow), Aleph-Hub (Next.js/TypeScript pipeline).

## Global Constraints

- **MSRV 1.95**; toolchain pinned to stable `1.96.0` via `rust-toolchain.toml`. Bash tool is non-interactive — prepend `export PATH="/opt/homebrew/opt/rustup/bin:/opt/homebrew/bin:$PATH"` before any `cargo`/`just`.
- **极度节制 cargo**: default no full test runs; at most **one** `cargo check -p alephcore --lib` per high-risk merge. Prefer `cargo check` over `cargo build`. Run `cargo fmt -p alephcore` before each Rust commit.
- **No new dependencies** — git2, include_dir, serde, reqwest, zip are all already in `Cargo.toml`. Do NOT add async-std/smol, a vector DB client, platform-API crates, or non-serde serialization (CLAUDE.md Do-NOT list).
- **Redlines:** R3 core minimalism, R4 interfaces are pure I/O, R7/P8 no regex/rule-engine for semantic decisions, R10 thin harness — this work adds no reasoning to `src/harness/`.
- **Owner = `rootazero`**; repos public; runtime clone tracks `main`; repo root = current dir contents (skill/plugin leaves at root, no nested `skills/`/`plugins/` level).
- **Commit format:** `<scope>: <description>` (English). Attribution disabled — no Co-Authored-By trailer.
- **Branch:** work directly on `main` (project single-branch mode).
- **Official content repo URLs (constants):** `https://github.com/rootazero/Aleph-skills`, `https://github.com/rootazero/Aleph-plugins`.
- **Name-collision policy:** official sync only ever touches `SkillOrigin::Official` entries; it skips `Github`/`Local` (user/third-party wins).
- `docs/superpowers/**` is gitignored — this plan and the spec live on disk, not in git (repo convention).

---

# Phase A — Migration + Submodule + Build/Release Wiring

Operational phase. Establishes the two repos as the source of truth and re-wires the main repo to embed from submodules. Must complete before Phase B/C. Deliverable: `aleph-server` still builds and embeds identical official content, now sourced from submodules.

### Task A1: Create and push `Aleph-skills`

**Files:**
- Source: `/Volumes/TBU4/Workspace/Aleph/skills/` (37 leaves + `.gitignore`)
- Target: `/Volumes/TBU4/Workspace/Aleph-skills/` (existing empty sibling dir)

**Interfaces:**
- Produces: GitHub repo `rootazero/Aleph-skills` whose **root** = current `skills/` contents.

- [ ] **Step 1: Verify `gh` auth and the empty target**

Run:
```bash
gh auth status && ls -A /Volumes/TBU4/Workspace/Aleph-skills
```
Expected: authenticated as an account that can create repos under `rootazero`; target dir empty.

- [ ] **Step 2: Copy current skills content into the target (exclude any VCS cruft)**

```bash
rsync -a --exclude='.git' /Volumes/TBU4/Workspace/Aleph/skills/ /Volumes/TBU4/Workspace/Aleph-skills/
ls -A /Volumes/TBU4/Workspace/Aleph-skills | head
```
Expected: 37 skill dirs + `.gitignore` present at the target root.

- [ ] **Step 3: Add a minimal README so the repo root is self-describing**

Create `/Volumes/TBU4/Workspace/Aleph-skills/README.md`:
```markdown
# Aleph Skills

Official skills for [Aleph](https://github.com/rootazero/Aleph). Each top-level
directory is one skill (contains a `SKILL.md`). Curated into the Aleph Hub
catalog and installable via `aleph skills sync` or the Hub.
```

- [ ] **Step 4: Init, commit, create remote, push**

```bash
cd /Volumes/TBU4/Workspace/Aleph-skills
git init -b main
git add -A
git commit -m "skills: initial import from Aleph main repo snapshot"
gh repo create rootazero/Aleph-skills --public --source=. --remote=origin --push
```
Expected: repo created; `main` pushed.

- [ ] **Step 5: Verify remote**

Run: `gh repo view rootazero/Aleph-skills --json name,visibility,defaultBranchRef`
Expected: `name=Aleph-skills`, `visibility=PUBLIC`, default branch `main`.

### Task A2: Create and push `Aleph-plugins`

**Files:**
- Source: `/Volumes/TBU4/Workspace/Aleph/plugins/` (8 plugin leaves + `.claude-plugin/marketplace.toml` + `.gitignore`)
- Target: `/Volumes/TBU4/Workspace/Aleph-plugins/`

**Interfaces:**
- Produces: GitHub repo `rootazero/Aleph-plugins` whose **root** = current `plugins/` contents (including `.claude-plugin/marketplace.toml`).

- [ ] **Step 1: Copy current plugins content into the target**

```bash
rsync -a --exclude='.git' /Volumes/TBU4/Workspace/Aleph/plugins/ /Volumes/TBU4/Workspace/Aleph-plugins/
ls -A /Volumes/TBU4/Workspace/Aleph-plugins
test -f /Volumes/TBU4/Workspace/Aleph-plugins/.claude-plugin/marketplace.toml && echo "marketplace.toml OK"
```
Expected: 8 plugin dirs + `.claude-plugin/` + `.gitignore`; `marketplace.toml OK`.

- [ ] **Step 2: Add README**

Create `/Volumes/TBU4/Workspace/Aleph-plugins/README.md`:
```markdown
# Aleph Plugins

Official plugins (marketplace) for [Aleph](https://github.com/rootazero/Aleph).
`.claude-plugin/marketplace.toml` is the marketplace manifest; each other
top-level directory is one plugin. Listed in the Aleph Hub catalog.
```

- [ ] **Step 3: Init, commit, create remote, push**

```bash
cd /Volumes/TBU4/Workspace/Aleph-plugins
git init -b main
git add -A
git commit -m "plugins: initial import from Aleph main repo snapshot"
gh repo create rootazero/Aleph-plugins --public --source=. --remote=origin --push
```

- [ ] **Step 4: Verify remote**

Run: `gh repo view rootazero/Aleph-plugins --json name,visibility,defaultBranchRef`
Expected: public repo, default branch `main`.

### Task A3: Replace main-repo `skills/`+`plugins/` with submodules; fix `build.rs` rerun triggers

**Files:**
- Modify: `/Volumes/TBU4/Workspace/Aleph/.gitmodules` (create)
- Modify: `/Volumes/TBU4/Workspace/Aleph/build.rs` (add rerun-if-changed)
- Replace (as submodules): `skills/`, `plugins/`

**Interfaces:**
- Produces: `skills/` and `plugins/` checked out as submodules at the same paths; `include_dir!("$CARGO_MANIFEST_DIR/skills"|"plugins")` unchanged and compiling.

- [ ] **Step 1: Remove the now-duplicated dirs from the main repo index (keep working tree until submodule re-adds)**

```bash
cd /Volumes/TBU4/Workspace/Aleph
git rm -r --cached skills plugins
rm -rf skills plugins
```

- [ ] **Step 2: Add the two repos as submodules at the same paths**

```bash
git submodule add https://github.com/rootazero/Aleph-skills.git skills
git submodule add https://github.com/rootazero/Aleph-plugins.git plugins
git submodule update --init --recursive
ls skills | head -3 && ls plugins | head -3
```
Expected: `.gitmodules` created; `skills/` and `plugins/` populated from the new repos.

- [ ] **Step 3: Add `rerun-if-changed` for the embedded trees in `build.rs`**

In `/Volumes/TBU4/Workspace/Aleph/build.rs`, inside `fn main()` right after the `VERSION` rerun line, add:
```rust
    // Re-embed bundled skills/plugins (include_dir!) when their submodule
    // checkouts change — without this, edits land only after a clean build.
    println!("cargo:rerun-if-changed=skills");
    println!("cargo:rerun-if-changed=plugins");
```

- [ ] **Step 4: Compile-check that `include_dir!` still resolves against the submodule paths**

Run:
```bash
export PATH="/opt/homebrew/opt/rustup/bin:/opt/homebrew/bin:$PATH"
cargo check -p alephcore --lib
```
Expected: clean (exit 0). `BUNDLED_SKILLS`/`BUNDLED_PLUGINS` embed from the submodule dirs.

- [ ] **Step 5: Commit**

```bash
cargo fmt -p alephcore
git add .gitmodules skills plugins build.rs
git commit -m "build: source bundled skills/plugins from submodules + rerun triggers"
```

### Task A4: Submodule bump in `just release` + `submodules: recursive` on CI checkouts

**Rationale (corrected from initial draft):** the release builds in a 3-platform matrix job (`desktop-app`), so a bump+commit+push *inside* CI would race across the parallel jobs and also miss the triggering checkout ref. Instead the bump runs **once, locally, in the `just release` recipe** (a single recorded commit pushed to `main` before the workflow triggers), and CI only needs `submodules: recursive` on every checkout. Without `submodules: recursive`, CI checks out empty `skills/`/`plugins/` → `include_dir!` embeds nothing → broken release. That checkout flag is the real must-have; the local bump is the D5 recorded-pointer guarantee.

**Files:**
- Modify: `/Volumes/TBU4/Workspace/Aleph/justfile` (`release` recipe; `verify-build` recipe)
- Modify: `/Volumes/TBU4/Workspace/Aleph/.github/workflows/aleph-app-release.yml` (checkouts at lines ~41, ~228)
- Modify: `/Volumes/TBU4/Workspace/Aleph/.github/workflows/aleph-core-ci.yml` (checkouts at lines ~41, ~82, ~130, ~160, ~186)

**Interfaces:**
- Produces: each release embeds latest-at-release official content via a recorded submodule-pointer bump (D5); every CI/release build fetches submodule content so `include_dir!` is non-empty.

- [ ] **Step 1: Add `submodules: recursive` to every `actions/checkout@v6` in both workflows**

Each checkout step is currently the bare line `      - uses: actions/checkout@v6`. Replace **each** occurrence in both files with:
```yaml
      - uses: actions/checkout@v6
        with:
          submodules: recursive
```
(If a checkout step already has a `with:` block, add `submodules: recursive` as another key under it instead of duplicating `with:`.)

- [ ] **Step 2: Add the submodule bump to the `just release` recipe (local, recorded)**

In `justfile`, in the `release` recipe, replace this block:
```bash
    # Stage, commit, push
    git add -f VERSION Cargo.toml CHANGELOG.md
    git commit -m "release: v${VERSION}"
    git push origin main
```
with:
```bash
    # Bump bundled skills/plugins submodules to their latest upstream main so
    # this release embeds the newest official content (the offline fallback).
    git submodule update --remote --recursive

    # Stage, commit, push (submodule pointer bump rides along, recorded in the release commit)
    git add -f VERSION Cargo.toml CHANGELOG.md
    git add skills plugins
    git commit -m "release: v${VERSION}"
    git push origin main
```

- [ ] **Step 3: Ensure `verify-build` initializes submodules (fresh-clone safety)**

In `justfile`, in the `verify-build` recipe, add as its first command (after the shebang/`set` if present, before the build steps):
```bash
    git submodule update --init --recursive
```

- [ ] **Step 4: Verify YAML parses and justfile lists recipes**

Run:
```bash
python3 -c "import yaml; [yaml.safe_load(open(f)) for f in ['.github/workflows/aleph-app-release.yml','.github/workflows/aleph-core-ci.yml']]; print('yaml OK')"
just --list >/dev/null && echo "justfile OK"
```
Expected: `yaml OK` and `justfile OK`.

- [ ] **Step 5: Commit**

```bash
git add justfile .github/workflows/aleph-app-release.yml .github/workflows/aleph-core-ci.yml
git commit -m "ci: fetch submodules on checkout + bump bundled content in just release"
```

---

# Phase B — Runtime Bootstrap/Sync (clone-with-fallback + explicit refresh)

Rust TDD phase. Adds: a filesystem-source extraction path symmetric to the embedded one; a git2 clone/fetch sync module; first-run clone-with-fallback orchestration; an explicit `bundled.sync` RPC + CLI.

### Task B1: Filesystem-source extraction (symmetric to the embedded `Dir` path)

**Files:**
- Modify: `/Volumes/TBU4/Workspace/Aleph/src/bundled/extractor.rs`
- Test: same file, `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `InstallRegistry`, `SkillEntry`, `SkillOrigin` (from `src/bundled/manifest.rs`); `BUNDLED_VERSION`.
- Produces: `pub(crate) fn extract_skill_tree_from_dir(src_root: &Path, skills_dir: &Path, manifest: &mut InstallRegistry) -> bool` and `pub(crate) fn extract_plugins_from_dir(src_root: &Path, cache_dir: &Path) -> bool` — fs-source analogues of the existing `extract_skills`/`extract_plugins`, reusing `extract_dir_recursive` semantics via a new fs copy with prune.

- [ ] **Step 1: Write the failing test for fs-source skill extraction + collision skip**

Add to the `tests` module in `extractor.rs`:
```rust
    #[test]
    fn extract_skill_tree_from_dir_copies_official_and_skips_user_owned() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        let skills = tmp.path().join("skills");
        std::fs::create_dir_all(src.join("api-design")).unwrap();
        std::fs::write(src.join("api-design").join("SKILL.md"), b"# api").unwrap();
        std::fs::create_dir_all(src.join("search")).unwrap();
        std::fs::write(src.join("search").join("SKILL.md"), b"# official search").unwrap();
        std::fs::create_dir_all(&skills).unwrap();

        // User already owns a skill named "search" → official sync must skip it.
        let mut manifest = InstallRegistry::new("");
        manifest.skills.insert(
            "search".to_string(),
            SkillEntry { source: SkillOrigin::Local, version: None, url: None, installed_at: None },
        );

        let ok = extract_skill_tree_from_dir(&src, &skills, &mut manifest);
        assert!(ok);
        assert!(skills.join("api-design").join("SKILL.md").exists(), "official extracted");
        assert!(!skills.join("search").exists(), "user-owned name skipped");
        assert_eq!(manifest.skills.get("api-design").unwrap().source, SkillOrigin::Official);
        assert_eq!(manifest.skills.get("search").unwrap().source, SkillOrigin::Local);
    }
```

- [ ] **Step 2: Run it to confirm it fails to compile (function missing)**

Run: `export PATH="/opt/homebrew/opt/rustup/bin:/opt/homebrew/bin:$PATH" && cargo test -p alephcore --lib bundled::extractor 2>&1 | head -20`
Expected: FAIL — `cannot find function extract_skill_tree_from_dir`.

- [ ] **Step 3: Implement the fs-source extractors (reusing manifest/skip/prune logic)**

Add to `extractor.rs` (above the `tests` module):
```rust
/// Filesystem-source analogue of `extract_skills`: walks a cloned checkout
/// (each top-level dir is one skill) and applies the SAME manifest gating —
/// official-only overwrite, skip names owned by non-official skills, prune
/// stale files within each official skill dir. Returns true if all extractions
/// succeeded.
pub(crate) fn extract_skill_tree_from_dir(
    src_root: &Path,
    skills_dir: &Path,
    manifest: &mut InstallRegistry,
) -> bool {
    let mut all_ok = true;
    let entries = match std::fs::read_dir(src_root) {
        Ok(e) => e,
        Err(e) => {
            warn!(error = %e, src = %src_root.display(), "Failed to read cloned skills checkout");
            return false;
        }
    };
    for entry in entries.filter_map(|e| e.ok()) {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue; // skip top-level files (.gitignore, README.md, .git)
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || name.contains('/') || name.contains('\\') || name.len() > 255 {
            continue;
        }
        // Skip if a non-official skill already owns this name (user/3rd-party wins).
        if let Some(e) = manifest.skills.get(&name) {
            if e.source != SkillOrigin::Official {
                debug!(skill = %name, source = ?e.source, "Skipping user-owned skill name");
                continue;
            }
        }
        let target = skills_dir.join(&name);
        match copy_tree_with_prune(&entry.path(), &target) {
            Ok(()) => {
                manifest.skills.insert(
                    name.clone(),
                    SkillEntry {
                        source: SkillOrigin::Official,
                        version: Some(BUNDLED_VERSION.to_string()),
                        url: None,
                        installed_at: None,
                    },
                );
            }
            Err(e) => {
                manifest.skills.insert(
                    name.clone(),
                    SkillEntry { source: SkillOrigin::Official, version: None, url: None, installed_at: None },
                );
                warn!(skill = %name, error = %e, "Failed to extract skill from checkout");
                all_ok = false;
            }
        }
    }
    all_ok
}

/// Filesystem-source analogue of `extract_plugins`: atomically swap the cloned
/// plugins checkout into the official marketplace cache dir.
pub(crate) fn extract_plugins_from_dir(src_root: &Path, cache_dir: &Path) -> bool {
    let tmp_dir = cache_dir.with_extension("tmp");
    if tmp_dir.exists() {
        let _ = std::fs::remove_dir_all(&tmp_dir);
    }
    if let Err(e) = std::fs::create_dir_all(&tmp_dir) {
        warn!(error = %e, "Failed to create plugin cache temp dir");
        return false;
    }
    if let Err(e) = copy_dir_into(src_root, &tmp_dir) {
        warn!(error = %e, "Failed to copy plugins from checkout");
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return false;
    }
    swap_dir_into_place(&tmp_dir, cache_dir)
}

/// Recursively copy `src` → `dst`, skipping VCS metadata, then prune any
/// entries in `dst` no longer present in `src` (mirrors `prune_stale_entries`).
fn copy_tree_with_prune(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    copy_dir_into(src, dst)?;
    use std::collections::HashSet;
    let mut keep: HashSet<std::ffi::OsString> = HashSet::new();
    for e in std::fs::read_dir(src)?.filter_map(|e| e.ok()) {
        if !is_vcs_meta(&e.file_name()) {
            keep.insert(e.file_name());
        }
    }
    for e in std::fs::read_dir(dst)?.filter_map(|e| e.ok()) {
        if !keep.contains(&e.file_name()) {
            let p = e.path();
            let ft = e.file_type()?;
            if ft.is_dir() {
                let _ = std::fs::remove_dir_all(&p);
            } else {
                let _ = std::fs::remove_file(&p);
            }
        }
    }
    Ok(())
}

/// Recursively copy directory contents (no prune), skipping `.git`/`.gitignore`.
fn copy_dir_into(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)?.filter_map(|e| e.ok()) {
        let name = entry.file_name();
        if is_vcs_meta(&name) {
            continue;
        }
        let from = entry.path();
        let to = dst.join(&name);
        if entry.file_type()?.is_dir() {
            copy_dir_into(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

fn is_vcs_meta(name: &std::ffi::OsStr) -> bool {
    matches!(name.to_string_lossy().as_ref(), ".git" | ".gitignore" | ".gitmodules")
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p alephcore --lib bundled::extractor`
Expected: PASS (including the existing `swap_dir_*` tests).

- [ ] **Step 5: Commit**

```bash
cargo fmt -p alephcore
git add src/bundled/extractor.rs
git commit -m "bundled: add filesystem-source extraction symmetric to embedded path"
```

### Task B2: git2 sync module (clone-or-hard-reset isolated checkout)

**Files:**
- Create: `/Volumes/TBU4/Workspace/Aleph/src/bundled/sync.rs`
- Modify: `/Volumes/TBU4/Workspace/Aleph/src/bundled/mod.rs` (add `mod sync;` + URL/path constants + re-export)
- Test: `sync.rs` `#[cfg(test)] mod tests`

**Interfaces:**
- Produces: `pub(crate) fn clone_or_update(repo_url: &str, checkout_dir: &Path) -> Result<(), String>` — clones `main` if absent, else fetches + hard-resets to `origin/main`. Blocking network I/O (call under `spawn_blocking`).
- Produces constants in `mod.rs`: `OFFICIAL_SKILLS_REPO`, `OFFICIAL_PLUGINS_REPO`.

- [ ] **Step 1: Write the failing test using a local source repo (git2 can clone `file://`/path)**

Create `src/bundled/sync.rs` with only the test first:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// Build a tiny local git repo with one commit on `main` and return its path.
    fn make_source_repo(dir: &std::path::Path, content: &str) -> String {
        let repo = git2::Repository::init(dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), content).unwrap();
        let mut idx = repo.index().unwrap();
        idx.add_path(std::path::Path::new("SKILL.md")).unwrap();
        idx.write().unwrap();
        let tree = repo.find_tree(idx.write_tree().unwrap()).unwrap();
        let sig = git2::Signature::now("t", "t@t").unwrap();
        repo.commit(Some("refs/heads/main"), &sig, &sig, "init", &tree, &[]).unwrap();
        repo.set_head("refs/heads/main").unwrap();
        dir.to_string_lossy().to_string()
    }

    #[test]
    fn clone_then_update_pulls_latest() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        let url = make_source_repo(&src, "v1");
        let checkout = tmp.path().join("checkout");

        clone_or_update(&url, &checkout).expect("clone");
        assert_eq!(std::fs::read_to_string(checkout.join("SKILL.md")).unwrap(), "v1");

        // New commit upstream, then update → hard reset picks it up.
        let repo = git2::Repository::open(&src).unwrap();
        std::fs::write(src.join("SKILL.md"), "v2").unwrap();
        let mut idx = repo.index().unwrap();
        idx.add_path(std::path::Path::new("SKILL.md")).unwrap();
        idx.write().unwrap();
        let tree = repo.find_tree(idx.write_tree().unwrap()).unwrap();
        let sig = git2::Signature::now("t", "t@t").unwrap();
        let parent = repo.head().unwrap().peel_to_commit().unwrap();
        repo.commit(Some("refs/heads/main"), &sig, &sig, "v2", &tree, &[&parent]).unwrap();

        clone_or_update(&url, &checkout).expect("update");
        assert_eq!(std::fs::read_to_string(checkout.join("SKILL.md")).unwrap(), "v2");
    }

    #[test]
    fn clone_unreachable_remote_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let checkout = tmp.path().join("checkout");
        let err = clone_or_update("/nonexistent/repo/path", &checkout);
        assert!(err.is_err());
    }
}
```

- [ ] **Step 2: Run to confirm failure (function missing)**

Run: `cargo test -p alephcore --lib bundled::sync 2>&1 | head -20`
Expected: FAIL — `cannot find function clone_or_update`.

- [ ] **Step 3: Implement `clone_or_update` (mirrors the proven git2 pattern in `markdown_skills.rs`, using a canonical hard reset)**

Prepend to `sync.rs` (above the tests):
```rust
//! Official-content git sync — clone the external skills/plugins repos into an
//! isolated checkout, or hard-reset an existing checkout to `origin/main`.
//! Uses git2 (libgit2, vendored) so no system `git` is required. Network I/O is
//! blocking — call from `spawn_blocking`. Never panics; returns `Err` so the
//! caller can fall back to the embedded snapshot.

use std::path::Path;
use tracing::{info, warn};

/// Clone `repo_url` (branch `main`) into `checkout_dir` if absent; otherwise
/// fetch and hard-reset the working tree to `origin/main`. The checkout dir is
/// official-content-only and never user-edited, so a hard reset is conflict-free.
pub(crate) fn clone_or_update(repo_url: &str, checkout_dir: &Path) -> Result<(), String> {
    if checkout_dir.join(".git").exists() {
        return update_existing(checkout_dir).map_err(|e| format!("update {repo_url}: {e}"));
    }
    if let Some(parent) = checkout_dir.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    info!(url = %repo_url, dest = %checkout_dir.display(), "Cloning official content");
    git2::Repository::clone(repo_url, checkout_dir)
        .map(|_| ())
        .map_err(|e| format!("clone {repo_url}: {e}"))
}

fn update_existing(checkout_dir: &Path) -> Result<(), git2::Error> {
    let repo = git2::Repository::open(checkout_dir)?;
    let mut remote = repo.find_remote("origin")?;
    remote.fetch(&["main"], None, None)?;
    let fetch_head = repo.find_reference("FETCH_HEAD")?;
    let target = repo.reference_to_annotated_commit(&fetch_head)?.id();
    let obj = repo.find_object(target, None)?;
    // Canonical `reset --hard` — discard any local drift, match origin/main.
    repo.reset(&obj, git2::ResetType::Hard, None)?;
    if let Ok(mut head) = repo.find_reference("refs/heads/main") {
        let _ = head.set_target(target, "sync");
    }
    Ok(())
}
```

- [ ] **Step 4: Register the module + constants in `mod.rs`**

In `src/bundled/mod.rs`, after the existing `pub mod manifest;` line add `mod sync;` and after `BUNDLED_VERSION` add:
```rust
/// Upstream repos for the official content (runtime clone source; the embedded
/// snapshot above is the offline fallback, sourced from the same repos via
/// git submodules at build time).
pub const OFFICIAL_SKILLS_REPO: &str = "https://github.com/rootazero/Aleph-skills";
pub const OFFICIAL_PLUGINS_REPO: &str = "https://github.com/rootazero/Aleph-plugins";

pub(crate) use sync::clone_or_update;
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p alephcore --lib bundled::sync`
Expected: PASS (both tests).

- [ ] **Step 6: Commit**

```bash
cargo fmt -p alephcore
git add src/bundled/sync.rs src/bundled/mod.rs
git commit -m "bundled: add git2 clone/hard-reset sync for official content"
```

### Task B3: Orchestrate first-run clone-with-fallback; add explicit re-extract entry point

**Files:**
- Modify: `/Volumes/TBU4/Workspace/Aleph/src/bundled/extractor.rs`
- Modify: `/Volumes/TBU4/Workspace/Aleph/src/bin/aleph-server/commands/start/helpers.rs:259`
- Test: `extractor.rs` tests

**Interfaces:**
- Consumes: `clone_or_update`, `OFFICIAL_SKILLS_REPO`, `OFFICIAL_PLUGINS_REPO`, `extract_skill_tree_from_dir`, `extract_plugins_from_dir`.
- Produces: `pub fn sync_official_now(aleph_home: &Path, kind: SyncKind) -> Result<SyncReport, String>` (delegates to `sync_official_with_urls` with the real constants). `pub(crate) fn sync_official_with_urls(aleph_home: &Path, kind: SyncKind, skills_url: &str, plugins_url: &str) -> Result<SyncReport, String>` (URL-injectable core, for deterministic tests). `pub enum SyncKind { Skills, Plugins, All }`, `pub struct SyncReport { pub skills: bool, pub plugins: bool }`.
- Behavior change: `extract_bundled_content` is unchanged for the App-upgrade path (embedded re-extract on version change). First-run bootstrap (skills dir absent/empty) additionally **tries a clone first, falling back to embedded** on any error.

> **Execution note (controller):** per the project's cargo-batching constraint, the implementer does NOT run cargo. Skip Steps 2 and 5's commands; transcribe the test+code, self-review by reading, and commit. The controller runs the consolidated `cargo test -p alephcore --lib` at the Phase B boundary.

- [ ] **Step 1: Write the (deterministic, network-free) tests using the URL-injectable core**

Add to `extractor.rs` tests:
```rust
    /// Build a local git repo containing a single skill leaf `<leaf>/SKILL.md`.
    fn make_skill_repo(dir: &std::path::Path, leaf: &str, body: &str) -> String {
        std::fs::create_dir_all(dir.join(leaf)).unwrap();
        std::fs::write(dir.join(leaf).join("SKILL.md"), body).unwrap();
        let repo = git2::Repository::init(dir).unwrap();
        let mut idx = repo.index().unwrap();
        idx.add_all(["."], git2::IndexAddOption::DEFAULT, None).unwrap();
        idx.write().unwrap();
        let tree = repo.find_tree(idx.write_tree().unwrap()).unwrap();
        let sig = git2::Signature::now("t", "t@t").unwrap();
        repo.commit(Some("refs/heads/main"), &sig, &sig, "init", &tree, &[]).unwrap();
        dir.to_string_lossy().to_string()
    }

    #[test]
    fn sync_official_with_urls_extracts_skills_from_local_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        let url = make_skill_repo(&src, "api-design", "# api");
        let home = tmp.path().join("home");

        let report = sync_official_with_urls(&home, SyncKind::Skills, &url, "").expect("ok");
        assert!(report.skills);
        assert!(home.join("skills").join("api-design").join("SKILL.md").exists());
    }

    #[test]
    fn sync_official_with_urls_reports_err_on_bad_url() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let report = sync_official_with_urls(&home, SyncKind::Skills, "/nonexistent/repo/path", "");
        assert!(report.is_err());
    }
```

- [ ] **Step 2: (controller runs cargo at phase boundary — skip)**

- [ ] **Step 3: Implement `SyncKind`/`SyncReport`/`sync_official_with_urls`/`sync_official_now` and the first-run clone-with-fallback hook**

Add to `extractor.rs`:
```rust
/// Which official content to refresh.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncKind {
    Skills,
    Plugins,
    All,
}

/// Result of an explicit sync.
#[derive(Debug, Clone, Copy)]
pub struct SyncReport {
    pub skills: bool,
    pub plugins: bool,
}

/// Explicit refresh (the `bundled.sync` RPC / CLI) against the official repos.
pub fn sync_official_now(aleph_home: &Path, kind: SyncKind) -> Result<SyncReport, String> {
    sync_official_with_urls(
        aleph_home,
        kind,
        crate::bundled::OFFICIAL_SKILLS_REPO,
        crate::bundled::OFFICIAL_PLUGINS_REPO,
    )
}

/// URL-injectable core of the explicit refresh. Clones each requested repo's
/// latest `main` into an isolated checkout under `~/.aleph/cache/`, then
/// re-extracts via the fs-source path. Returns `Err` if every requested clone
/// failed (nothing was refreshed). Injectable URLs keep this deterministically
/// testable with local fixture repos (no network).
pub(crate) fn sync_official_with_urls(
    aleph_home: &Path,
    kind: SyncKind,
    skills_url: &str,
    plugins_url: &str,
) -> Result<SyncReport, String> {
    let skills_dir = aleph_home.join("skills");
    let plugins_cache = aleph_home.join("plugins").join("cache").join("aleph-official");
    let cache = aleph_home.join("cache");
    let _ = std::fs::create_dir_all(&skills_dir);
    let _ = std::fs::create_dir_all(&plugins_cache);

    let mut report = SyncReport { skills: false, plugins: false };
    let mut last_err: Option<String> = None;

    if matches!(kind, SyncKind::Skills | SyncKind::All) {
        let checkout = cache.join("aleph-skills-checkout");
        match crate::bundled::clone_or_update(skills_url, &checkout) {
            Ok(()) => {
                let mut manifest = InstallRegistry::load(&skills_dir)
                    .unwrap_or_else(|| InstallRegistry::new(""));
                let _ = manifest.reconcile(&skills_dir);
                report.skills = extract_skill_tree_from_dir(&checkout, &skills_dir, &mut manifest);
                let _ = manifest.reconcile(&skills_dir);
                let _ = manifest.save(&skills_dir);
            }
            Err(e) => last_err = Some(e),
        }
    }
    if matches!(kind, SyncKind::Plugins | SyncKind::All) {
        let checkout = cache.join("aleph-plugins-checkout");
        match crate::bundled::clone_or_update(plugins_url, &checkout) {
            Ok(()) => report.plugins = extract_plugins_from_dir(&checkout, &plugins_cache),
            Err(e) => last_err = Some(e),
        }
    }

    if !report.skills && !report.plugins {
        return Err(last_err.unwrap_or_else(|| "nothing synced".into()));
    }
    Ok(report)
}
```

Then, in `extract_bundled_content`, locate the first-run branch (the `None =>` arm of `InstallRegistry::load`, "No skills manifest found, performing initial reconcile"). Immediately after that arm builds `m` and BEFORE the `manifest.bundled_version == BUNDLED_VERSION` check, add a clone-with-fallback attempt:
```rust
    // First-run bootstrap: try to pull the latest official content from the
    // external repos; on any failure fall back to the embedded snapshot below.
    let first_run = manifest.skills.is_empty() && manifest.bundled_version.is_empty();
    if first_run {
        match sync_official_now(aleph_home, SyncKind::All) {
            Ok(r) => {
                info!(skills = r.skills, plugins = r.plugins, "Bootstrapped official content from remote");
                if r.skills && r.plugins {
                    if let Some(m) = InstallRegistry::load(&skills_dir) { manifest = m; }
                    manifest.bundled_version = BUNDLED_VERSION.to_string();
                    let _ = manifest.reconcile(&skills_dir);
                    let _ = manifest.save(&skills_dir);
                    cleanup_legacy_dir(aleph_home);
                    return;
                }
            }
            Err(e) => warn!(error = %e, "Remote bootstrap failed, falling back to embedded snapshot"),
        }
    }
```
(The existing embedded-extraction code remains below as the fallback and the App-upgrade path.)

- [ ] **Step 4: Make the startup call non-blocking (clone is network I/O)**

In `helpers.rs`, replace line 259 `alephcore::bundled::extract_bundled_content(&aleph_home);` with:
```rust
    // Bundled extraction may perform a one-time network clone on first run;
    // run it off the async executor so a slow clone never stalls startup.
    let home_for_extract = aleph_home.clone();
    let _ = tokio::task::spawn_blocking(move || {
        alephcore::bundled::extract_bundled_content(&home_for_extract)
    })
    .await;
```

- [ ] **Step 5: (controller runs `cargo test -p alephcore --lib` at the Phase B boundary — skip)**

- [ ] **Step 6: Commit**

```bash
git add src/bundled/extractor.rs src/bin/aleph-server/commands/start/helpers.rs
git commit -m "bundled: first-run clone-with-fallback + explicit sync entry point"
```
(Run `cargo fmt -p alephcore` only if available without a full compile; otherwise the controller formats at the phase boundary.)

### Task B4: `bundled.sync` RPC handler

**Files:**
- Create: `/Volumes/TBU4/Workspace/Aleph/src/gateway/handlers/bundled_sync.rs`
- Modify: `/Volumes/TBU4/Workspace/Aleph/src/gateway/handlers/mod.rs` (register + `mod`)
- Modify: `/Volumes/TBU4/Workspace/Aleph/src/gateway/rate_limiter.rs` (RpcWrite scope + list)
- Modify: `/Volumes/TBU4/Workspace/Aleph/src/gateway/lane.rs` (System lane)

**Interfaces:**
- Consumes: `alephcore::bundled::{sync_official_now, SyncKind}`, `alephcore::utils::paths::get_config_dir`.
- Produces: RPC method `bundled.sync`, params `{ "kind"?: "skills"|"plugins"|"all" (default "all") }`, result `{ "ok": bool, "skills": bool, "plugins": bool }`.

- [ ] **Step 1: Write the failing test (param parsing → SyncKind)**

Create `src/gateway/handlers/bundled_sync.rs`:
```rust
//! `bundled.sync` — explicitly refresh official skills/plugins from the
//! external repos (clone latest `main` → re-extract). Reserved for explicit
//! triggers (CLI / LLM tool / Hub button); the startup path never auto-pulls.

use crate::bundled::SyncKind;
use crate::gateway::handlers::parse_params;
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR};
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Deserialize)]
pub struct SyncParams {
    #[serde(default = "default_kind")]
    pub kind: String,
}

fn default_kind() -> String {
    "all".to_string()
}

pub(crate) fn parse_kind(s: &str) -> Option<SyncKind> {
    match s {
        "skills" => Some(SyncKind::Skills),
        "plugins" => Some(SyncKind::Plugins),
        "all" => Some(SyncKind::All),
        _ => None,
    }
}

pub async fn handle_sync(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: SyncParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let Some(kind) = parse_kind(&params.kind) else {
        return JsonRpcResponse::error(request.id, INTERNAL_ERROR, "invalid kind".to_string());
    };
    let aleph_home = match crate::utils::paths::get_config_dir() {
        Ok(p) => p,
        Err(e) => return JsonRpcResponse::error(request.id, INTERNAL_ERROR, e.to_string()),
    };
    match tokio::task::spawn_blocking(move || crate::bundled::sync_official_now(&aleph_home, kind)).await {
        Ok(Ok(r)) => JsonRpcResponse::success(
            request.id,
            json!({ "ok": true, "skills": r.skills, "plugins": r.plugins }),
        ),
        Ok(Err(e)) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, e),
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, format!("sync task failed: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parse_kind_maps_known_values() {
        assert!(matches!(parse_kind("skills"), Some(SyncKind::Skills)));
        assert!(matches!(parse_kind("plugins"), Some(SyncKind::Plugins)));
        assert!(matches!(parse_kind("all"), Some(SyncKind::All)));
        assert!(parse_kind("bogus").is_none());
    }
}
```

- [ ] **Step 2: Export `sync_official_now`/`SyncKind`/`SyncReport` from the bundled module**

In `src/bundled/mod.rs`, update the extractor re-export line to also export the new items:
```rust
pub use extractor::{extract_bundled_content, sync_official_now, SyncKind, SyncReport};
```

- [ ] **Step 3: Register the handler + module**

In `src/gateway/handlers/mod.rs`: add `pub mod bundled_sync;` near the other `pub mod` lines, and add next to the `skills.*` registrations (around line 425):
```rust
        registry.register("bundled.sync", bundled_sync::handle_sync);
```
Add to the registration test block (around line 1055):
```rust
        assert!(registry.has_method("bundled.sync"));
```

- [ ] **Step 4: Classify the method as a write (rate limit + lane)**

In `src/gateway/rate_limiter.rs:414`, extend the `RpcWrite` arm to include `"bundled.sync"`; in the method list near line 588 add `"bundled.sync",`.
In `src/gateway/lane.rs:108`, add `"bundled.sync" => Some(Self::System),`.

- [ ] **Step 5: Run the unit test + scoped check**

Run:
```bash
cargo test -p alephcore --lib gateway::handlers::bundled_sync
cargo check -p alephcore --lib
```
Expected: PASS; check exit 0.

- [ ] **Step 6: Commit**

```bash
cargo fmt -p alephcore
git add src/gateway/handlers/bundled_sync.rs src/gateway/handlers/mod.rs src/gateway/rate_limiter.rs src/gateway/lane.rs src/bundled/mod.rs
git commit -m "gateway: add bundled.sync RPC for explicit official-content refresh"
```

### Task B5: CLI `aleph skills sync` / `aleph plugins sync`

**Files:**
- Modify: `/Volumes/TBU4/Workspace/Aleph/interfaces/cli/src/commands/skills_cmd.rs` (add `sync` fn)
- Modify: `/Volumes/TBU4/Workspace/Aleph/interfaces/cli/src/commands/cli_args.rs` (add `Sync` to `enum SkillsAction` ~line 761 AND `enum PluginAction` ~line 628)
- Modify: `/Volumes/TBU4/Workspace/Aleph/interfaces/cli/src/main.rs` (dispatch arms: `dispatch_skills` ~lines 707-709 and `dispatch_plugin` ~lines 579-640)

**Interfaces:**
- Consumes: `bundled.sync` RPC; `AlephClient`.
- Produces: `aleph skills sync` (kind=skills) and `aleph plugin sync` (kind=plugins).

> **Execution note (controller):** per the cargo-batching constraint, the implementer does NOT run cargo. Skip Step 4's `cargo check`; the controller runs `cargo check -p aleph-cli` at the Phase B boundary.

- [ ] **Step 1: Add the `sync` CLI function to `skills_cmd.rs`**

```rust
/// Refresh official content from the external repos via `bundled.sync`.
pub async fn sync(server_url: &str, kind: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;
    let params = serde_json::json!({ "kind": kind });
    let result: Value = client.call("bundled.sync", Some(params)).await?;
    if json {
        output::print_json(&result);
    } else {
        let s = result.get("skills").and_then(|v| v.as_bool()).unwrap_or(false);
        let p = result.get("plugins").and_then(|v| v.as_bool()).unwrap_or(false);
        println!("Synced official content (skills={s}, plugins={p}).");
    }
    client.close().await?;
    Ok(())
}
```

- [ ] **Step 2: Add the `Sync` variant to BOTH action enums (cli_args.rs)**

In `enum SkillsAction` (~line 761), add:
```rust
    /// Refresh official skills from the external repo (git clone/pull)
    Sync,
```
In `enum PluginAction` (~line 628), add (alongside `Update`/`Reload` etc.):
```rust
    /// Refresh official plugins from the external repo (git clone/pull)
    Sync,
```

- [ ] **Step 3: Dispatch the new arms in `main.rs`**

In `dispatch_skills` (the `match action` over `SkillsAction`, ~lines 707-709), add:
```rust
        SkillsAction::Sync => skills_cmd::sync(server_url, "skills", json).await,
```
In `dispatch_plugin` (the `match action` over `PluginAction`, ~lines 579-640), add:
```rust
        PluginAction::Sync => skills_cmd::sync(server_url, "plugins", json).await,
```
(`skills_cmd` is already imported/used in `dispatch_skills`; if `dispatch_plugin` is in the same `main.rs` it can call `skills_cmd::sync` directly — `skills_cmd` is a sibling module under `commands`. Confirm the module path resolves, e.g. `crate::commands::skills_cmd::sync` if a bare `skills_cmd::` isn't in scope in `dispatch_plugin`.)

- [ ] **Step 4: (controller runs `cargo check -p aleph-cli` at the Phase B boundary — skip)**

- [ ] **Step 5: Commit**

```bash
git add interfaces/cli/src/commands/skills_cmd.rs interfaces/cli/src/commands/cli_args.rs interfaces/cli/src/main.rs
git commit -m "cli: add 'skills sync' / 'plugin sync' for official-content refresh"
```
(Run `cargo fmt -p aleph-cli` only if available without a full compile; otherwise the controller formats at the phase boundary.)

---

# Phase C — Hub Integration

Adds the missing `GitDir → skill` install branch in the client (so the Hub can install official/third-party *skills*, not just plugins) and re-points the Aleph-Hub first-party seed at the new repos.

### Task C1: `GitDir → skill` install branch in the hub installer

**Files:**
- Modify: `/Volumes/TBU4/Workspace/Aleph/src/hub/install.rs`
- Test: `src/hub/install.rs` `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `InstallSpec::GitDir { git_url, subdir, git_ref, .. }`, `ExtensionEntry { kind, name, .. }`, `ExtensionKind::Skill`, `alephcore::bundled::clone_or_update`, `InstallRegistry`/`SkillOrigin` from `crate::bundled::manifest`.
- Produces: `InstallOutcome::Skill { path: String }`; `run_install` routes `GitDir` to the skill path when `ctx.entry.kind == ExtensionKind::Skill`, else the existing plugin path. New pure helper `install_git_skill(entry, spec, skills_dir) -> Result<String, String>`.

- [ ] **Step 1: Write the failing test (kind=Skill + local source repo → skill lands + stamped Github)**

Add to `install.rs` tests (build a local source repo whose `<subdir>` holds a `SKILL.md`):
```rust
    #[test]
    fn install_git_skill_clones_subdir_and_stamps_source() {
        use crate::hub::types::{ExtensionCategory, ExtensionEntry, ExtensionKind, InstallSpec, TrustTier};
        let tmp = tempfile::tempdir().unwrap();
        // Source repo with a `my-skill/SKILL.md` leaf.
        let src = tmp.path().join("src");
        std::fs::create_dir_all(src.join("my-skill")).unwrap();
        std::fs::write(src.join("my-skill").join("SKILL.md"), b"# hi").unwrap();
        let repo = git2::Repository::init(&src).unwrap();
        let mut idx = repo.index().unwrap();
        idx.add_all(["."], git2::IndexAddOption::DEFAULT, None).unwrap();
        idx.write().unwrap();
        let tree = repo.find_tree(idx.write_tree().unwrap()).unwrap();
        let sig = git2::Signature::now("t", "t@t").unwrap();
        repo.commit(Some("refs/heads/main"), &sig, &sig, "init", &tree, &[]).unwrap();

        let skills_dir = tmp.path().join("skills");
        std::fs::create_dir_all(&skills_dir).unwrap();
        let entry = ExtensionEntry {
            id: "aleph-hub:x/my-skill".into(), kind: ExtensionKind::Skill,
            category: ExtensionCategory::Developer, name: "my-skill".into(),
            description: "d".into(), author: None, icon: None, tags: vec![], version: None,
            source_id: "aleph-hub".into(), repo_url: None, trust_tier: TrustTier::Community,
            requires_config: false, config_schema: None, installed: false,
            install_spec: None,
        };
        let spec = InstallSpec::GitDir {
            git_url: src.to_string_lossy().to_string(),
            subdir: Some("my-skill".into()), git_ref: Some("main".into()), sha256: None,
        };
        let path = install_git_skill(&entry, &spec, &skills_dir).expect("install");
        assert!(std::path::Path::new(&path).join("SKILL.md").exists());
        let manifest = crate::bundled::manifest::InstallRegistry::load(&skills_dir).unwrap();
        assert_eq!(
            manifest.skills.get("my-skill").unwrap().source,
            crate::bundled::manifest::SkillOrigin::Github
        );
    }
```
(Note: confirm the `ExtensionEntry` field set against `src/hub/types.rs` at implementation time and adjust literal fields to match exactly — the struct continues past the excerpt in the spec.)

- [ ] **Step 2: Run to confirm failure (function missing)**

Run: `cargo test -p alephcore --lib hub::install::tests::install_git_skill 2>&1 | head -20`
Expected: FAIL — `cannot find function install_git_skill`.

- [ ] **Step 3: Implement `install_git_skill` + route in `run_install`**

Add to `install.rs`:
```rust
/// Install a single skill from a `GitDir` spec: clone the repo into an isolated
/// checkout, copy the `<subdir>` leaf into `~/.aleph/skills/<name>`, and stamp
/// it `Github` in the manifest (so official sync never overwrites it). Pure
/// w.r.t. the gateway — takes the resolved skills dir.
pub fn install_git_skill(
    entry: &crate::hub::types::ExtensionEntry,
    spec: &InstallSpec,
    skills_dir: &std::path::Path,
) -> Result<String, String> {
    let InstallSpec::GitDir { git_url, subdir, .. } = spec else {
        return Err("install_git_skill requires a GitDir spec".into());
    };
    let leaf = subdir.clone().unwrap_or_else(|| entry.name.clone());
    let safe_name = leaf.rsplit('/').next().unwrap_or(&leaf).to_string();
    if safe_name.is_empty() || safe_name.contains("..") {
        return Err(format!("unsafe skill name '{safe_name}'"));
    }
    // Clone into an isolated per-source checkout (never the live skills dir).
    let checkout = skills_dir.join(".git-cache").join(crate::hub::install::mcp_server_id(&entry.id));
    crate::bundled::clone_or_update(git_url, &checkout)?;
    let src_leaf = checkout.join(&leaf);
    if !src_leaf.is_dir() {
        return Err(format!("subdir '{leaf}' not found in {git_url}"));
    }
    let target = skills_dir.join(&safe_name);
    crate::bundled::copy_skill_leaf(&src_leaf, &target).map_err(|e| e.to_string())?;

    // Stamp manifest as Github so official sync skips it.
    let mut manifest = crate::bundled::manifest::InstallRegistry::load(skills_dir)
        .unwrap_or_else(|| crate::bundled::manifest::InstallRegistry::new(""));
    manifest.skills.insert(
        safe_name.clone(),
        crate::bundled::manifest::SkillEntry {
            source: crate::bundled::manifest::SkillOrigin::Github,
            version: entry.version.clone(),
            url: Some(git_url.clone()),
            installed_at: None,
        },
    );
    let _ = manifest.save(skills_dir);
    Ok(target.display().to_string())
}
```
Expose the copy helper from the bundled module: in `src/bundled/extractor.rs` rename-or-wrap `copy_tree_with_prune` as `pub(crate) fn copy_skill_leaf` (or add `pub(crate) fn copy_skill_leaf(src: &Path, dst: &Path) -> std::io::Result<()> { copy_tree_with_prune(src, dst) }`) and re-export `pub(crate) use extractor::copy_skill_leaf;` plus make `mcp_server_id` `pub(crate)` in `install.rs`.

Add `Skill` to `InstallOutcome`:
```rust
    Skill { path: String },
```
In `run_install`, change the `GitDir` arm to branch on kind:
```rust
        InstallSpec::GitDir { .. } => {
            if ctx.entry.kind == crate::hub::types::ExtensionKind::Skill {
                let skills_dir = crate::utils::paths::get_config_dir()
                    .map_err(|e| e.to_string())?
                    .join("skills");
                let path = install_git_skill(ctx.entry, spec, &skills_dir)?;
                Ok(InstallOutcome::Skill { path })
            } else {
                // existing plugin marketplace path (unchanged)
                let marketplace = ctx.marketplace.ok_or("marketplace unavailable")?;
                let marketplace_name =
                    (ctx.entry.source_id != "local").then_some(ctx.entry.source_id.as_str());
                let path = marketplace.install_to_scope(&ctx.entry.name, marketplace_name, PluginScope::User, None)?;
                Ok(InstallOutcome::Plugin { path: path.display().to_string() })
            }
        }
```
Update `outcome_json` in `src/gateway/handlers/extensions/install.rs` to handle `InstallOutcome::Skill { path } => json!({ "kind": "skill", "path": path })`, and `verify_install` to treat `Skill` like `Plugin` (reload extension manager).

- [ ] **Step 4: Run the test + scoped check**

Run:
```bash
cargo test -p alephcore --lib hub::install
cargo check -p alephcore --lib
```
Expected: PASS; check exit 0.

- [ ] **Step 5: Commit**

```bash
cargo fmt -p alephcore
git add src/hub/install.rs src/bundled/extractor.rs src/bundled/mod.rs src/gateway/handlers/extensions/install.rs
git commit -m "hub: install GitDir skills into ~/.aleph/skills (close skill-install gap)"
```

### Task C2: Re-point the Aleph-Hub first-party seed at the new repos

**Files (in `/Volumes/TBU4/Workspace/Aleph-Hub`):**
- Modify: `data/seeds/aleph-official.json`
- Regenerate: `public/catalog.json`, `data/site-catalog.json`

**Interfaces:**
- Produces: catalog entries whose official-skill `install_spec` is `git_dir { git_url: Aleph-skills, subdir: <leaf> }` (root layout).

- [ ] **Step 1: Update the `groups` block**

In `data/seeds/aleph-official.json`, set:
```jsonc
"skill": {
  "git_url": "https://github.com/rootazero/Aleph-skills",
  "git_ref": "main",
  "subdir_prefix": "",
  "tree_url": "https://github.com/rootazero/Aleph-skills/tree/main",
  "owner": "rootazero", "repo": "Aleph-skills",
  "stars": 0, "license": "MIT", "updated": "2026-06-21"
},
"plugin": {
  "git_url": "https://github.com/rootazero/Aleph-plugins",
  "git_ref": "main",
  "subdir_prefix": "",
  "tree_url": "https://github.com/rootazero/Aleph-plugins/tree/main",
  "owner": "rootazero", "repo": "Aleph-plugins",
  "stars": 0, "license": null, "updated": "2026-06-21"
}
```

- [ ] **Step 2: Confirm `firstParty.ts` handles an empty `subdir_prefix`**

Read `scripts/pipeline/firstParty.ts`: `toFinal` builds `subdir = `${g.subdir_prefix}/${e.leaf}``. With `subdir_prefix=""` this yields `"/api-design"` (leading slash). Fix the join to drop an empty prefix:
```ts
  const subdir = g.subdir_prefix ? `${g.subdir_prefix}/${e.leaf}` : e.leaf;
```
(Apply the same to the `repo_url`/`tree_url` construction so links resolve to `.../tree/main/<leaf>`.)

- [ ] **Step 3: Regenerate artifacts**

Run (in Aleph-Hub):
```bash
cd /Volumes/TBU4/Workspace/Aleph-Hub
npx tsx scripts/pipeline/regen-firstparty.ts
```
Expected: `regen-firstparty: N entries (43 official + … )`.

- [ ] **Step 4: Verify the official install_spec now points at the new repo**

Run: `python3 -c "import json;d=json.load(open('public/catalog.json'));e=[x for x in d['entries'] if x['id'].endswith('/api-design')][0];print(e['install_spec'])"`
Expected: `git_dir` with `git_url` ending `Aleph-skills` and `subdir: "api-design"`.

- [ ] **Step 5: Commit (in Aleph-Hub)**

```bash
git add data/seeds/aleph-official.json scripts/pipeline/firstParty.ts public/catalog.json data/site-catalog.json
git commit -m "seed: point first-party skills/plugins at the external repos (root layout)"
git push
```

### Task C3 (optional / fast-follow): Panel per-entry "Update official content" button

**Files:**
- Modify: the Leptos Panel hub view (`interfaces/webchat/src/...` — locate the extensions/hub view).

**Interfaces:**
- Consumes: `bundled.sync` RPC.
- Produces: a button in the Hub view that calls `bundled.sync { kind: "all" }` and toasts the result.

- [ ] **Step 1: Add a "Refresh official content" action wired to `bundled.sync`** (follow the existing RPC-call pattern in the panel's hub view; pass `{ "kind": "all" }`).
- [ ] **Step 2: Rebuild WASM + redeploy** per `DESKTOP_SHELL.md` (`just wasm` → recompile `aleph-server` → swap the running binary). Verify the served WASM sha matches `dist/`.
- [ ] **Step 3: Commit** the panel change + rebuilt `dist/` artifacts.

> If deferring, the LLM (`bundled.sync` tool) and CLI (`aleph skills sync`) already provide explicit refresh — the button is UX polish.

---

## Self-Review

**1. Spec coverage:**
- Spec D1 (bulk clone + fallback) → B3 first-run clone-with-fallback. ✓
- D2 (reuse extractor, swap source) → B1 fs-source extraction reusing manifest/reconcile/swap. ✓
- D3 (explicit sync) → B4 RPC + B5 CLI + C3 button; startup never auto-pulls (B3 only first-run + version-gate). ✓
- D4 (fresh snapshot, create+push repos) → A1/A2. ✓
- D5 (submodule + release bump) → A3 + A4. ✓
- D6 (App upgrade → embedded) → B3 leaves the version-gate embedded path unchanged. ✓
- §4 conflict handling (isolated checkout, hard reset, manifest-gated, name-collision skip) → B2 (`reset --hard`), B1 (skip non-Official), C1 (stamp Github). ✓
- §5.D9 (GitDir→skill gap) → C1. ✓
- §5.D8 (seed git_url/subdir_prefix) → C2. ✓
- §5.B (build.rs rerun) → A3 Step 3. ✓
- §5.11 (verify `plugins-index.json`) → covered as a note; see Open item below.

**2. Placeholder scan:** No TBD/TODO; every code step shows full code. C1 Step 1 carries an explicit "confirm `ExtensionEntry` field set" instruction (the struct continues past the spec excerpt) — this is a real verification, not a placeholder. C3 is explicitly optional.

**3. Type consistency:** `clone_or_update(repo_url, checkout_dir) -> Result<(), String>` used identically in B2/B3/C1. `SyncKind`/`SyncReport`/`sync_official_now` defined in B3, consumed in B4. `extract_skill_tree_from_dir`/`extract_plugins_from_dir` defined B1, consumed B3. `copy_skill_leaf` (wrapping `copy_tree_with_prune`) defined B1/C1. `InstallOutcome::Skill` added C1 and consumed in `outcome_json`/`verify_install`. `SkillOrigin::{Official,Github,Local}` matches `manifest.rs` verbatim. ✓

**Open item (carry into execution):** verify whether `plugins-index.json` (main repo root) is still consumed; if dead, remove it in a small follow-up commit; if live, update its `download_url`s. Not blocking any task above.

---

**Plan complete and saved to `docs/superpowers/plans/2026-06-21-skills-plugins-external-repos.md`.**
