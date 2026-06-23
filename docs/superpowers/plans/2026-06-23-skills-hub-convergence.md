# Skills → Aleph Hub Convergence Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the Aleph Hub the single authority for official skills — every bundled official skill appears in the Hub catalog as `aleph-hub:<skill_id>` / `Official`, shows accurate installed status, and reinstalls through the existing Hub engine.

**Architecture:** Primer-only, mirroring the merged MCP convergence. A new `src/hub/official_skills.rs` projects the compile-time-embedded `BUNDLED_SKILLS` tree into `ExtensionEntry`s; a new `src/hub/primer.rs` holds the unified cold-start gate that composes the MCP + skill projections into one `replace_source` on the shared `aleph-hub` slot. The existing name-based `mark_installed` already collapses each primer entry against the live installed skill, so **no reconcile change, no migration, no RPC retirement, and no Panel changes** are needed.

**Tech Stack:** Rust (alephcore lib), `include_dir` (compile-time embed), `serde`, `tokio`, in-binary skill frontmatter parser (`crate::skill::manifest::parse_skill_content`).

## Global Constraints

- **Canonical id = `format!("aleph-hub:{}", manifest.id())`** — `manifest.id()` is the `SkillId` derived from the frontmatter `name` (lowercased, spaces→hyphens, collapsed). It may differ from the bundle directory name.
- **`GitDir.subdir` = the bundle directory name**, carried separately from the slug (decoupled).
- **Exactly one cold-start primer call at boot**, composing MCP + skills into a single `replace_source(ALEPH_HUB_ID, …)`. A second per-kind primer on the same slot would clobber the first (`replace_source` is wholesale).
- **Do NOT modify** `src/hub/reconcile.rs`, `src/hub/install.rs`, any `skills.*` gateway handler, or the Panel. Mirrors MCP's "reconcile 字节不动".
- **Projected entries:** `kind = ExtensionKind::Skill`, `category = ExtensionCategory::Other`, `trust_tier = TrustTier::Official`, `source_id = via = ALEPH_HUB_ID`, `installed/enabled/update_available = false` (flipped later by `mark_installed`), `install_spec = GitDir { git_url: OFFICIAL_SKILLS_REPO, subdir: Some(<dir_name>), git_ref: None, sha256: None }`.
- **Tests must be submodule-independent.** `skills/` is a git submodule that may be empty in dev/CI; `BUNDLED_SKILLS` is then an empty `Dir`. No test may assert that a specific bundled skill exists. The MCP `catalog.json` is always present, so MCP entries are the stable, non-zero anchor for primer tests.
- **Redlines:** R3 (core minimalism — no new heavy deps; reuse `include_dir`, `parse_skill_content`), R10 (do not touch `src/harness/`), single-source design (no peer source, no local dedup; the remote fetch overwrites the slot wholesale).
- **cargo restraint (project rule):** run only the targeted single-`-C`-package, single-module test named in each task. Do NOT run the full test suite. Multiple filters go AFTER `--` (e.g. `cargo test -p alephcore --lib -- a::b c::d`); a second positional before `--` is an arg-parse error.

---

### Task 1: Project bundled skills into Hub catalog entries (`official_skills.rs`)

**Files:**
- Create: `src/hub/official_skills.rs`
- Modify: `src/hub/mod.rs` (register the module)
- Test: `src/hub/official_skills.rs` (inline `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `crate::bundled::{BUNDLED_SKILLS, OFFICIAL_SKILLS_REPO}` (`BUNDLED_SKILLS.dirs()` / `dir.files()` / `file.contents_utf8()` — inherent `include_dir` methods, no trait import); `crate::domain::skill::{SkillManifest, SkillSource}` + `crate::domain::Entity` (the latter brings `manifest.id() -> &SkillId`; `manifest.name()`/`description()` are inherent; `SkillId: Display`); `crate::skill::manifest::parse_skill_content(&str, SkillSource) -> Result<SkillManifest, SkillParseError>`; `crate::hub::catalog_client::ALEPH_HUB_ID`; `crate::hub::types::{ExtensionCategory, ExtensionEntry, ExtensionKind, InstallSpec, TrustTier}`.
- Produces: `pub fn primer_entries() -> Vec<crate::hub::types::ExtensionEntry>` and (module-private) `fn project_skill(dir_name: &str, manifest: &SkillManifest) -> ExtensionEntry`. Consumed by Task 2.

- [ ] **Step 1: Register the module and add the test (failing).**

Add to `src/hub/mod.rs`, in the `pub mod …;` block (alphabetical, after `official_mcp`):

```rust
pub mod official_skills;
```

Create `src/hub/official_skills.rs` with imports + the test module only (the functions are referenced but not yet defined — this is the RED state):

```rust
//! Cold-start projection of bundled official skills into Hub catalog entries.
//!
//! Projects the compile-time-embedded `BUNDLED_SKILLS` tree into `ExtensionEntry`s
//! for the `aleph-hub` source slot (consumed by `hub::primer`) so official skills
//! are browsable/installable offline and before the remote catalog is fetched.
//! The remote fetch later overwrites the slot wholesale (no peer source, no dedup).

use crate::bundled::{BUNDLED_SKILLS, OFFICIAL_SKILLS_REPO};
use crate::domain::skill::{SkillManifest, SkillSource};
use crate::domain::Entity; // brings `manifest.id()` into scope (status.rs does the same)
use crate::hub::catalog_client::ALEPH_HUB_ID;
use crate::hub::types::{ExtensionCategory, ExtensionEntry, ExtensionKind, InstallSpec, TrustTier};
use crate::skill::manifest::parse_skill_content;

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "---\nname: PDF Tools\ndescription: Work with PDFs.\n---\nBody.";

    fn manifest_from(md: &str) -> SkillManifest {
        parse_skill_content(md, SkillSource::Bundled).expect("sample SKILL.md parses")
    }

    #[test]
    fn project_skill_yields_official_aleph_hub_entry() {
        let m = manifest_from(SAMPLE);
        // dir_name deliberately differs from the SkillId ("pdf-tools") to lock decoupling.
        let e = project_skill("pdf-tools-dir", &m);
        assert_eq!(e.id, "aleph-hub:pdf-tools");
        assert_eq!(e.kind, ExtensionKind::Skill);
        assert_eq!(e.category, ExtensionCategory::Other);
        assert_eq!(e.trust_tier, TrustTier::Official);
        assert_eq!(e.source_id, "aleph-hub");
        assert_eq!(e.via.as_deref(), Some("aleph-hub"));
        assert_eq!(e.name, "PDF Tools");
        assert!(!e.installed);
        match e.install_spec.unwrap() {
            InstallSpec::GitDir { git_url, subdir, git_ref, sha256 } => {
                assert_eq!(git_url, OFFICIAL_SKILLS_REPO);
                assert_eq!(subdir.as_deref(), Some("pdf-tools-dir"));
                assert!(git_ref.is_none() && sha256.is_none());
            }
            other => panic!("expected GitDir, got {other:?}"),
        }
        assert!(!e.requires_config);
    }

    #[test]
    fn primer_entries_tolerates_absent_bundle() {
        // The skills submodule may be empty in dev/CI; primer_entries must not
        // panic, and whatever it returns must be well-formed official skills.
        let entries = primer_entries();
        for e in &entries {
            assert_eq!(e.kind, ExtensionKind::Skill);
            assert_eq!(e.trust_tier, TrustTier::Official);
            assert!(e.id.starts_with("aleph-hub:"));
        }
    }
}
```

- [ ] **Step 2: Run the test — verify it fails to compile.**

Run: `cargo test -p alephcore --lib hub::official_skills`
Expected: FAIL — `cannot find function 'project_skill' in this scope` / `cannot find function 'primer_entries'`.

- [ ] **Step 3: Implement the projection.**

Insert these functions into `src/hub/official_skills.rs`, **above** the `#[cfg(test)] mod tests` block:

```rust
/// Project one bundled skill into a Hub catalog entry. `dir_name` is the bundle
/// directory name (== the Aleph-skills repo subdir); the canonical slug is the
/// manifest's `SkillId` (frontmatter-name-derived), which may differ from it.
fn project_skill(dir_name: &str, manifest: &SkillManifest) -> ExtensionEntry {
    let spec = InstallSpec::GitDir {
        git_url: OFFICIAL_SKILLS_REPO.to_string(),
        subdir: Some(dir_name.to_string()),
        git_ref: None,
        sha256: None,
    };
    ExtensionEntry {
        id: format!("{ALEPH_HUB_ID}:{}", manifest.id()),
        kind: ExtensionKind::Skill,
        category: ExtensionCategory::Other,
        name: manifest.name().to_string(),
        description: manifest.description().to_string(),
        author: None,
        icon: None,
        tags: vec![ExtensionKind::Skill.as_str().to_string()],
        version: None,
        source_id: ALEPH_HUB_ID.to_string(),
        repo_url: Some(OFFICIAL_SKILLS_REPO.to_string()),
        trust_tier: TrustTier::Official,
        requires_config: spec.requires_config(),
        config_schema: None,
        installed: false,
        enabled: false,
        update_available: false,
        via: Some(ALEPH_HUB_ID.to_string()),
        install_spec: Some(spec),
    }
}

/// Project the in-binary bundled official skills into Hub catalog entries.
/// Returns `[]` (logged) when the `skills/` submodule was absent at build time.
pub fn primer_entries() -> Vec<ExtensionEntry> {
    let mut out = Vec::new();
    for dir in BUNDLED_SKILLS.dirs() {
        let Some(dir_name) = dir.path().file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        // The SKILL.md directly inside this skill dir (borrowed from the static embed).
        let Some(content) = dir
            .files()
            .find(|f| f.path().file_name().and_then(|n| n.to_str()) == Some("SKILL.md"))
            .and_then(|f| f.contents_utf8())
        else {
            continue;
        };
        match parse_skill_content(content, SkillSource::Bundled) {
            Ok(manifest) => out.push(project_skill(dir_name, &manifest)),
            Err(e) => {
                tracing::warn!(skill = %dir_name, error = %e, "primer: skipping unparseable bundled SKILL.md")
            }
        }
    }
    if out.is_empty() {
        tracing::info!("official skills primer: bundle empty (submodule absent at build) — no skill entries");
    }
    out
}
```

- [ ] **Step 4: Run the test — verify it passes.**

Run: `cargo test -p alephcore --lib hub::official_skills`
Expected: PASS (`2 passed`). `project_skill_yields_official_aleph_hub_entry` proves the projection shape and slug/subdir decoupling; `primer_entries_tolerates_absent_bundle` proves submodule-independence.

- [ ] **Step 5: Commit.**

```bash
git add src/hub/official_skills.rs src/hub/mod.rs
git commit -m "hub: project bundled official skills into Hub catalog entries"
```

---

### Task 2: Unified cold-start primer + boot wiring + installed-marking regression (`primer.rs`)

**Files:**
- Create: `src/hub/primer.rs`
- Modify: `src/hub/mod.rs` (register `primer`)
- Modify: `src/hub/official_mcp.rs` (remove the cold-start gate `prime_official_mcp_if_empty` + its moved test; update the module doc; keep MCP projection + legacy migration)
- Modify: `src/bin/aleph-server/commands/start/mod.rs:822` (call the unified primer)
- Modify: `src/gateway/handlers/extensions/catalog.rs` (tests only — add two Skill-kind `mark_installed` regression tests)
- Test: `src/hub/primer.rs` + `src/gateway/handlers/extensions/catalog.rs`

**Interfaces:**
- Consumes: `crate::hub::official_mcp::primer_entries()` (kept), `crate::hub::official_skills::primer_entries()` (Task 1), `crate::hub::cache::{CatalogCache, CatalogFilter}`, `crate::hub::catalog_client::ALEPH_HUB_ID`, `crate::hub::types::ExtensionKind`.
- Produces: `pub async fn prime_official_catalog_if_empty(cache: &CatalogCache)`. Replaces `official_mcp::prime_official_mcp_if_empty` at the single boot call site.

- [ ] **Step 1: Register the module and write the primer tests (failing).**

Add to `src/hub/mod.rs`, in the `pub mod …;` block (after `official_skills`):

```rust
pub mod primer;
```

Create `src/hub/primer.rs` with imports + the test module only (the function is referenced but not yet defined — RED state):

```rust
//! Unified cold-start primer for the `aleph-hub` catalog slot.
//!
//! Composes the official MCP and official skill projections into a single
//! `replace_source` so neither clobbers the other (the slot is replace-based).
//! Runs only when the slot is empty (never fetched); the async remote fetch
//! later overwrites the slot wholesale.

use crate::hub::cache::CatalogCache;
use crate::hub::catalog_client::ALEPH_HUB_ID;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub::cache::CatalogFilter;
    use crate::hub::types::ExtensionKind;

    #[tokio::test]
    async fn primes_when_empty_then_is_noop_when_populated() {
        let cache = CatalogCache::open_in_memory().unwrap();
        prime_official_catalog_if_empty(&cache).await;
        let after = cache
            .query(&CatalogFilter { source_id: Some(ALEPH_HUB_ID.into()), ..Default::default() })
            .await
            .unwrap();
        // MCP catalog.json is always present, so the slot is non-empty even when
        // the skills submodule is absent.
        assert!(after.iter().any(|e| e.id == "aleph-hub:context7"));
        let count = after.len();
        // Second call is a no-op (slot already non-empty).
        prime_official_catalog_if_empty(&cache).await;
        let again = cache
            .query(&CatalogFilter { source_id: Some(ALEPH_HUB_ID.into()), ..Default::default() })
            .await
            .unwrap();
        assert_eq!(again.len(), count);
    }

    #[tokio::test]
    async fn skills_extension_does_not_clobber_mcp() {
        let cache = CatalogCache::open_in_memory().unwrap();
        prime_official_catalog_if_empty(&cache).await;
        let mcp = cache
            .query(&CatalogFilter { kind: Some(ExtensionKind::Mcp), ..Default::default() })
            .await
            .unwrap();
        // The full MCP primer set survives composition with the skills projection.
        assert_eq!(mcp.len(), crate::hub::official_mcp::primer_entries().len());
        assert!(mcp.iter().all(|e| e.kind == ExtensionKind::Mcp));
    }
}
```

- [ ] **Step 2: Run the primer tests — verify they fail to compile.**

Run: `cargo test -p alephcore --lib hub::primer`
Expected: FAIL — `cannot find function 'prime_official_catalog_if_empty' in this scope`.

- [ ] **Step 3: Implement the unified primer.**

Insert into `src/hub/primer.rs`, **above** the `#[cfg(test)] mod tests` block:

```rust
/// Cold-start primer: if the `aleph-hub` slot is empty (never fetched), fill it
/// with the official MCP + skill projections so official extensions are
/// available offline. The async remote fetch later `replace_source`s the slot.
pub async fn prime_official_catalog_if_empty(cache: &CatalogCache) {
    match cache.count_source(ALEPH_HUB_ID).await {
        Ok(0) => {
            let mut entries = crate::hub::official_mcp::primer_entries();
            entries.extend(crate::hub::official_skills::primer_entries());
            match cache.replace_source(ALEPH_HUB_ID, &entries).await {
                Ok(()) => tracing::info!(
                    count = entries.len(),
                    "primed official catalog (cold start: MCP + skills)"
                ),
                Err(e) => tracing::warn!(error = %e, "failed to prime official catalog"),
            }
        }
        Ok(_) => {}
        Err(e) => {
            tracing::warn!(error = %e, "count_source failed; skipping official catalog primer")
        }
    }
}
```

- [ ] **Step 4: Remove the now-duplicated cold-start gate from `official_mcp.rs`.**

In `src/hub/official_mcp.rs`, update the module doc comment (top of file) — replace:

```rust
//! Cold-start primer + legacy migration for official MCP presets.
//!
//! Projects the in-binary `src/mcp/presets/catalog.json` into `ExtensionEntry`s
//! under the `aleph-hub` source slot so official MCP is browsable/installable
//! offline and before the remote catalog is first fetched. The remote fetch
//! later overwrites the slot wholesale (no peer source, no local dedup).
```

with:

```rust
//! Official MCP preset projection + legacy migration.
//!
//! Projects the in-binary `src/mcp/presets/catalog.json` into `ExtensionEntry`s
//! for the `aleph-hub` source slot (consumed by `hub::primer`). Also migrates
//! servers installed via the retired preset path. The cold-start gate that
//! writes the slot lives in `hub::primer`.
```

Delete the `prime_official_mcp_if_empty` function entirely (the doc-commented `pub async fn prime_official_mcp_if_empty(cache: &crate::hub::cache::CatalogCache) { … }` block).

In the `#[cfg(test)] mod tests` of the same file, change the import line:

```rust
    use super::{is_legacy_preset_server, prime_official_mcp_if_empty, primer_entries};
```

to:

```rust
    use super::{is_legacy_preset_server, primer_entries};
```

and delete the moved test function in full (the `#[tokio::test] async fn primes_when_empty_then_is_noop_when_populated() { … }` block, including its local `use crate::hub::cache::{CatalogCache, CatalogFilter};`).

- [ ] **Step 5: Wire the boot call to the unified primer.**

In `src/bin/aleph-server/commands/start/mod.rs` around line 821-822, replace:

```rust
                // Cold-start: seed official MCP into the aleph-hub slot if empty.
                alephcore::hub::official_mcp::prime_official_mcp_if_empty(&cache).await;
```

with:

```rust
                // Cold-start: seed official catalog (MCP + skills) into the aleph-hub slot if empty.
                alephcore::hub::primer::prime_official_catalog_if_empty(&cache).await;
```

- [ ] **Step 6: Add the Skill-kind installed-marking regression tests.**

In `src/gateway/handlers/extensions/catalog.rs`, inside the existing `#[cfg(test)] mod tests` block (which already defines the `catalog_entry` and `installed_entry` helpers), append:

```rust
    #[test]
    fn skill_entry_marked_installed_by_name_case_insensitive() {
        // The primer's "aleph-hub:pdf-tools" Skill entry collapses against a live
        // local:skill entry of the same name — this is why official skills show
        // installed with NO reconcile change (the convergence's load-bearing fact).
        let mut catalog = vec![catalog_entry("aleph-hub:pdf-tools", ExtensionKind::Skill, "PDF Tools")];
        let installed = vec![installed_entry("local:skill:pdf-tools", ExtensionKind::Skill, "pdf tools", true)];
        mark_installed(&mut catalog, &installed);
        assert!(catalog[0].installed);
        assert!(catalog[0].enabled);
    }

    #[test]
    fn skill_entry_not_installed_when_name_differs() {
        let mut catalog = vec![catalog_entry("aleph-hub:pdf-tools", ExtensionKind::Skill, "PDF Tools")];
        let installed = vec![installed_entry("local:skill:other", ExtensionKind::Skill, "Other Skill", true)];
        mark_installed(&mut catalog, &installed);
        assert!(!catalog[0].installed);
    }
```

- [ ] **Step 7: Run the affected tests — verify they pass.**

Run: `cargo test -p alephcore --lib -- hub::primer hub::official_mcp gateway::handlers::extensions::catalog`
Expected: PASS. `hub::primer` (2 new): cold-start fills the slot, second call is a no-op, and `query(kind=Mcp)` still returns the full MCP set (skills never clobber MCP). `hub::official_mcp`: unchanged tests still pass after removing the moved gate test. `gateway::handlers::extensions::catalog`: the two new Skill tests pass (mark_installed already matches Skill by name — these characterize and lock that behavior), alongside the existing ones.

- [ ] **Step 8: Commit.**

```bash
git add src/hub/primer.rs src/hub/mod.rs src/hub/official_mcp.rs src/bin/aleph-server/commands/start/mod.rs src/gateway/handlers/extensions/catalog.rs
git commit -m "hub: unified cold-start primer composing official MCP + skills"
```

---

## Self-Review

**1. Spec coverage:**
- D1 (cold-start primer) → Task 1 (`primer_entries`) + Task 2 (`prime_official_catalog_if_empty`).
- D2 (source = whole bundled tree, via `parse_skill_content`) → Task 1 `primer_entries`.
- D3 (canonical id `aleph-hub:<skill_id>`, subdir = dir name) → Task 1 `project_skill` + its `project_skill_yields_official_aleph_hub_entry` test (dir_name ≠ skill_id case).
- D4 (unified primer, one `replace_source`, gate in `primer.rs`, one boot call) → Task 2 Steps 1-5 + `skills_extension_does_not_clobber_mcp`.
- D5 (no reconcile change) → not modified; proven load-bearing by the Task 2 Step 6 catalog tests.
- D6 (no migration) → no task; nothing added.
- D7 (no RPC retirement, no Panel changes) → no task; nothing added.
- D8 (reinstall via GitDir) → Task 1 `install_spec` = `GitDir{OFFICIAL_SKILLS_REPO, subdir}` (asserted in Task 1 test).
- §6 cross-repo id/name contract → verification only, user-owned, outside this repo (carried as an Open Item below).
- §8 testing (submodule-independent) → `primer_entries_tolerates_absent_bundle`, MCP-anchored primer tests, synthetic `project_skill`.

**2. Placeholder scan:** None. Every code step carries complete code; deletion steps name exact symbols + the exact import line to edit; the boot edit gives the exact before/after lines.

**3. Type consistency:** `primer_entries()` returns `Vec<ExtensionEntry>` in both modules and is composed in `prime_official_catalog_if_empty` via `entries.extend(...)`. `ALEPH_HUB_ID` used consistently. `project_skill(dir_name: &str, manifest: &SkillManifest)`; `manifest.id()`/`name()`/`description()` match the verified `SkillManifest`/`Entity` API. `InstallSpec::GitDir` field names (`git_url`, `subdir`, `git_ref`, `sha256`) match `types.rs`. `CatalogFilter` fields (`kind`, `source_id`, `..Default::default()`) match the usage in `catalog.rs` and `official_mcp.rs`. The boot path `alephcore::hub::primer::prime_official_catalog_if_empty` matches the new module registration.

## Open Items (carry to execution / PR)

1. **§6 cross-repo id/name contract (user-owned, outside this repo):** when the remote `hub.heyaleph.com` catalog ships official skills, each must carry `id = aleph-hub:<skill_id>` AND a `name` equal to the bundled skill's frontmatter `name` — skill installed-marking is name-based, so a mismatched name splits installed-status. Stricter than the MCP id-based contract. Align before the remote catalog ships skill entries.
2. **Submodule presence in release builds:** the primer only projects skills if the `skills/` submodule was initialized at build time (so `include_dir!` embeds them). Release/CI build pipelines must `git submodule update --init` — otherwise official skills silently won't appear in the cold-start catalog (graceful: they still show in `extensions.installed` as `local:skill:*`). Verify the release workflow initializes submodules.
