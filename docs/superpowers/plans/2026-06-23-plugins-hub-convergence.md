# Plugins → Aleph Hub Convergence Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make official plugins discoverable/installable from the Aleph Hub at cold start with Official identity, mirroring the MCP and skills convergences (primer-only) plus one surgical install-path fix.

**Architecture:** A new `src/hub/official_plugins.rs` projects the bundled `aleph-official` marketplace manifest into `aleph-hub:<name>` Official Plugin `ExtensionEntry`s; the unified cold-start primer composes them with MCP + skills into one `replace_source`. The Hub plugin-install branch is taught (via a pure helper) to resolve `aleph-hub`-sourced entries to the builtin `aleph-official` marketplace.

**Tech Stack:** Rust, `alephcore` lib, `include_dir!`-embedded `plugins/` tree, `toml` (via existing `parse_marketplace_toml_content`), `rusqlite` catalog cache.

**Spec:** `docs/superpowers/specs/2026-06-23-plugins-hub-convergence-design.md`

## Global Constraints

- **Single-slot design**: the `aleph-hub` source slot is replace-based; every primed entry MUST carry `source_id = "aleph-hub"` (`ALEPH_HUB_ID`) or the remote fetch's `replace_source` orphans it.
- **canonical id**: `id = format!("{ALEPH_HUB_ID}:{}", entry.name)`; `name = entry.name` (== plugin.toml name == live `PluginRecord.name`, the name-collapse contract).
- **No reconcile change** (`mark_installed` name-collapse already works — `plugin_entry_marked_installed_by_name_case_insensitive` exists), **no migration**, **no RPC retirement**, **no Panel changes**.
- **Install-path mapping (Option A)**: `source_id == ALEPH_HUB_ID` → marketplace `BUILTIN_MARKETPLACE_NAME` ("aleph-official"); `"local"` → None (search all); else verbatim.
- **GitDir is a routing marker for plugins**: the plugin install path reads the local marketplace cache by `name`; `git_url`/`subdir`/`sha256` are provenance only, NOT consumed there.
- **Submodule-independent tests**: `plugins/` may be empty in dev/CI → `BUNDLED_PLUGINS` may embed empty → `primer_entries()` returns `[]`. Projection tests use a synthetic marketplace.toml string; MCP `catalog.json` is the stable anchor.
- **Redlines**: R3 (no heavy deps — reuse existing parser), R10 (do not touch `src/harness/`), single-source (no peer source, no dedup).
- **cargo restraint**: run only the targeted lib tests named per task; multiple filters go AFTER `--`. No bin change in this plan → `cargo test --lib` fully covers it.
- **Commits**: English, format `<scope>: <description>`, no attribution footer.

---

### Task 1: `official_plugins.rs` — project bundled marketplace into Hub entries

**Files:**
- Create: `src/hub/official_plugins.rs`
- Modify: `src/hub/mod.rs` (register the module)
- Test: inline `#[cfg(test)] mod tests` in `src/hub/official_plugins.rs`

**Interfaces:**
- Consumes:
  - `crate::bundled::BUNDLED_PLUGINS` (`include_dir::Dir`), `crate::bundled::OFFICIAL_PLUGINS_REPO` (`&str`)
  - `crate::extension::marketplace::MarketplacePluginEntry` (fields: `name: String`, `source: String`, `description: Option<String>`, `version: Option<String>`, `sha256: Option<String>`)
  - `crate::extension::marketplace::manifest::parse_marketplace_toml_content(&str) -> Result<MarketplaceManifest, String>` (`MarketplaceManifest.plugins: Vec<MarketplacePluginEntry>`)
  - `crate::hub::catalog_client::ALEPH_HUB_ID` (`&str` = "aleph-hub")
  - `crate::hub::types::{ExtensionCategory, ExtensionEntry, ExtensionKind, InstallSpec, TrustTier}`; `ExtensionKind::as_str()`, `InstallSpec::requires_config()`
- Produces:
  - `pub fn primer_entries() -> Vec<ExtensionEntry>` (consumed by `hub::primer` in Task 2)
  - `fn project_plugin(entry: &MarketplacePluginEntry) -> ExtensionEntry` (module-private; tested)

- [ ] **Step 1: Write `official_plugins.rs` with projection + primer + tests**

Create `src/hub/official_plugins.rs`:

```rust
//! Cold-start projection of bundled official plugins into Hub catalog entries.
//!
//! Projects the bundled `aleph-official` marketplace manifest (embedded via
//! `BUNDLED_PLUGINS`) into `ExtensionEntry`s for the `aleph-hub` source slot
//! (consumed by `hub::primer`) so official plugins are browsable/installable
//! offline and before the remote catalog is fetched. The remote fetch later
//! overwrites the slot wholesale (no peer source, no dedup).

use crate::bundled::{BUNDLED_PLUGINS, OFFICIAL_PLUGINS_REPO};
use crate::extension::marketplace::manifest::parse_marketplace_toml_content;
use crate::extension::marketplace::MarketplacePluginEntry;
use crate::hub::catalog_client::ALEPH_HUB_ID;
use crate::hub::types::{ExtensionCategory, ExtensionEntry, ExtensionKind, InstallSpec, TrustTier};

/// Relative path of the marketplace manifest inside the bundled plugins tree.
const MARKETPLACE_TOML: &str = ".claude-plugin/marketplace.toml";

/// Project one marketplace plugin entry into a Hub catalog entry.
///
/// `source_id` is `aleph-hub` (slot correctness — the remote fetch refreshes the
/// slot by this key). The `GitDir` spec is a *routing marker* (makes `run_install`
/// take the plugin branch) plus provenance; the plugin install path reads the
/// local marketplace cache by `name`, so `git_url`/`subdir` are NOT consumed there.
fn project_plugin(entry: &MarketplacePluginEntry) -> ExtensionEntry {
    // The marketplace `source` is a "./<dir>" path relative to the marketplace
    // root; keep only the leaf for provenance.
    let subdir = entry
        .source
        .strip_prefix("./")
        .or_else(|| entry.source.strip_prefix('.'))
        .unwrap_or(&entry.source)
        .to_string();
    let spec = InstallSpec::GitDir {
        git_url: OFFICIAL_PLUGINS_REPO.to_string(),
        subdir: Some(subdir),
        git_ref: None,
        sha256: None,
    };
    ExtensionEntry {
        id: format!("{ALEPH_HUB_ID}:{}", entry.name),
        kind: ExtensionKind::Plugin,
        category: ExtensionCategory::Other,
        name: entry.name.clone(),
        description: entry.description.clone().unwrap_or_default(),
        author: None,
        icon: None,
        tags: vec![ExtensionKind::Plugin.as_str().to_string()],
        version: entry.version.clone(),
        source_id: ALEPH_HUB_ID.to_string(),
        repo_url: Some(OFFICIAL_PLUGINS_REPO.to_string()),
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

/// Project the in-binary bundled official marketplace's plugins into Hub entries.
/// Returns `[]` (logged) when the `plugins/` submodule was absent at build time
/// or the bundled manifest is missing/unparseable.
pub fn primer_entries() -> Vec<ExtensionEntry> {
    let Some(content) = BUNDLED_PLUGINS
        .get_file(MARKETPLACE_TOML)
        .and_then(|f| f.contents_utf8())
    else {
        tracing::info!(
            "official plugins primer: bundled marketplace manifest absent (submodule absent at build) — no plugin entries"
        );
        return Vec::new();
    };
    match parse_marketplace_toml_content(content) {
        Ok(manifest) => manifest.plugins.iter().map(project_plugin).collect(),
        Err(e) => {
            tracing::warn!(error = %e, "official plugins primer: failed to parse bundled marketplace manifest");
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_MANIFEST: &str = r#"
name = "aleph-official"

[[plugins]]
name = "diagnostics"
source = "./diagnostics"
description = "System health monitoring"
version = "0.1.0"

[[plugins]]
name = "diff-viewer"
source = "./diff-viewer"
"#;

    #[test]
    fn project_plugin_yields_official_aleph_hub_plugin_entry() {
        let manifest = parse_marketplace_toml_content(SAMPLE_MANIFEST).unwrap();
        let e = project_plugin(&manifest.plugins[0]);
        assert_eq!(e.id, "aleph-hub:diagnostics");
        assert_eq!(e.kind, ExtensionKind::Plugin);
        assert_eq!(e.category, ExtensionCategory::Other);
        assert_eq!(e.trust_tier, TrustTier::Official);
        assert_eq!(e.source_id, "aleph-hub");
        assert_eq!(e.via.as_deref(), Some("aleph-hub"));
        assert_eq!(e.name, "diagnostics");
        assert_eq!(e.description, "System health monitoring");
        assert_eq!(e.version.as_deref(), Some("0.1.0"));
        assert!(!e.installed);
        match e.install_spec.unwrap() {
            InstallSpec::GitDir { git_url, subdir, git_ref, sha256 } => {
                assert_eq!(git_url, OFFICIAL_PLUGINS_REPO);
                assert_eq!(subdir.as_deref(), Some("diagnostics"));
                assert!(git_ref.is_none() && sha256.is_none());
            }
            other => panic!("expected GitDir, got {other:?}"),
        }
        // GitDir requires no env config → plugin cards install with no config gate.
        assert!(!e.requires_config);
    }

    #[test]
    fn project_plugin_defaults_absent_description_and_version() {
        let manifest = parse_marketplace_toml_content(SAMPLE_MANIFEST).unwrap();
        // diff-viewer entry omits description and version.
        let e = project_plugin(&manifest.plugins[1]);
        assert_eq!(e.id, "aleph-hub:diff-viewer");
        assert_eq!(e.name, "diff-viewer");
        assert_eq!(e.description, "");
        assert!(e.version.is_none());
    }

    #[test]
    fn primer_entries_tolerates_absent_bundle() {
        // The plugins submodule may be empty in dev/CI; primer_entries must not
        // panic, and whatever it returns must be well-formed official plugins
        // anchored in the aleph-hub slot.
        let entries = primer_entries();
        for e in &entries {
            assert_eq!(e.kind, ExtensionKind::Plugin);
            assert_eq!(e.trust_tier, TrustTier::Official);
            assert_eq!(e.source_id, ALEPH_HUB_ID);
            assert!(e.id.starts_with("aleph-hub:"));
        }
    }
}
```

- [ ] **Step 2: Register the module in `src/hub/mod.rs`**

Insert `pub mod official_plugins;` between `pub mod official_mcp;` and `pub mod official_skills;` (lines 9–10):

```rust
pub mod official_mcp;
pub mod official_plugins;
pub mod official_skills;
```

- [ ] **Step 3: Run the tests — expect PASS**

Run: `cargo test -p alephcore --lib -- hub::official_plugins`
Expected: PASS (3 tests). Note: this is the first compile of the new module; if `parse_marketplace_toml_content` or `MarketplacePluginEntry` import paths are wrong, fix the `use` paths (they are re-exported as written: `MarketplacePluginEntry` at `crate::extension::marketplace`, the parser at `crate::extension::marketplace::manifest`).

- [ ] **Step 4: Commit**

```bash
git add src/hub/official_plugins.rs src/hub/mod.rs
git commit -m "hub: project bundled official plugins into Aleph Hub catalog entries"
```

---

### Task 2: Compose into the primer + fix the plugin install marketplace resolution

**Files:**
- Modify: `src/hub/primer.rs` (compose `official_plugins::primer_entries()`; doc + log)
- Modify: `src/hub/install.rs` (add `plugin_marketplace_name`; use it in the plugin branch)
- Test: inline `#[cfg(test)] mod tests` in both files

**Interfaces:**
- Consumes: `crate::hub::official_plugins::primer_entries()` (Task 1); `crate::hub::catalog_client::ALEPH_HUB_ID`; `crate::extension::marketplace::BUILTIN_MARKETPLACE_NAME` (`&str` = "aleph-official")
- Produces: no new public API; behavioral change only (primed plugin entries appear in the `aleph-hub` slot; `run_install` resolves `aleph-hub` plugin entries to the builtin marketplace)

- [ ] **Step 1: Add the failing install test (`plugin_marketplace_name`)**

In `src/hub/install.rs`, inside the existing `#[cfg(test)] mod tests { use super::*; ... }`, add:

```rust
    #[test]
    fn plugin_marketplace_name_maps_hub_to_builtin() {
        // Hub-official plugin entries (source_id == ALEPH_HUB_ID) install from the
        // builtin "aleph-official" marketplace — NOT a marketplace literally named
        // "aleph-hub" (which does not exist). "local" searches all marketplaces;
        // any other id is a registered peer marketplace, taken verbatim.
        assert_eq!(plugin_marketplace_name(ALEPH_HUB_ID), Some(BUILTIN_MARKETPLACE_NAME));
        assert_eq!(plugin_marketplace_name("aleph-hub"), Some("aleph-official"));
        assert_eq!(plugin_marketplace_name("local"), None);
        assert_eq!(plugin_marketplace_name("peer-market"), Some("peer-market"));
    }
```

- [ ] **Step 2: Run it to verify it fails (does not compile yet)**

Run: `cargo test -p alephcore --lib -- hub::install::tests::plugin_marketplace_name`
Expected: FAIL — `cannot find function plugin_marketplace_name` / `cannot find value ALEPH_HUB_ID` / `BUILTIN_MARKETPLACE_NAME` in this scope.

- [ ] **Step 3: Add the imports + pure helper in `src/hub/install.rs`**

Add to the imports block (after line 14, `use crate::hub::types::{ExtensionEntry, InstallSpec};`):

```rust
use crate::extension::marketplace::BUILTIN_MARKETPLACE_NAME;
use crate::hub::catalog_client::ALEPH_HUB_ID;
```

Add the helper just above `pub async fn run_install` (after the `install_git_skill` fn, ~line 143):

```rust
/// Resolve which marketplace an install entry's plugin lives in.
///
/// Hub-official plugin entries are primed with `source_id == ALEPH_HUB_ID`, but
/// the slot key is not a marketplace name — these plugins are bundled into the
/// builtin `aleph-official` marketplace, so they install from it. `"local"` means
/// "search all marketplaces by name"; any other source id is a registered peer
/// marketplace, taken verbatim.
fn plugin_marketplace_name(source_id: &str) -> Option<&str> {
    match source_id {
        ALEPH_HUB_ID => Some(BUILTIN_MARKETPLACE_NAME),
        "local" => None,
        other => Some(other),
    }
}
```

- [ ] **Step 4: Use the helper in the `run_install` plugin branch**

In `run_install`, replace the marketplace-name derivation (currently inside the `else` of the `InstallSpec::GitDir` arm):

Replace:
```rust
                let marketplace_name =
                    (ctx.entry.source_id != "local").then_some(ctx.entry.source_id.as_str());
```
with:
```rust
                let marketplace_name = plugin_marketplace_name(&ctx.entry.source_id);
```

(Leave the surrounding lines — `let marketplace = ctx.marketplace.ok_or(...)?;` and the `marketplace.install_to_scope(&ctx.entry.name, marketplace_name, PluginScope::User, None)?` call — unchanged.)

- [ ] **Step 5: Run the install test — expect PASS**

Run: `cargo test -p alephcore --lib -- hub::install::tests::plugin_marketplace_name`
Expected: PASS.

- [ ] **Step 6: Compose plugins into the primer (`src/hub/primer.rs`)**

Update the module doc (lines 3–4) to mention plugins:
```rust
//! Composes the official MCP, skill, and plugin projections into a single
//! `replace_source` so none clobbers the others (the slot is replace-based).
```

In `prime_official_catalog_if_empty`, add the third `extend` after the skills line:
```rust
            let mut entries = crate::hub::official_mcp::primer_entries();
            entries.extend(crate::hub::official_skills::primer_entries());
            entries.extend(crate::hub::official_plugins::primer_entries());
```

Update the success log message:
```rust
                Ok(()) => tracing::info!(
                    count = entries.len(),
                    "primed official catalog (cold start: MCP + skills + plugins)"
                ),
```

- [ ] **Step 7: Add the primer no-clobber test (`src/hub/primer.rs`)**

In the existing `#[cfg(test)] mod tests`, add:

```rust
    #[tokio::test]
    async fn plugins_compose_without_clobbering_mcp() {
        let cache = CatalogCache::open_in_memory().unwrap();
        prime_official_catalog_if_empty(&cache).await;
        // The full MCP set survives the three-way composition (catalog.json anchor).
        let mcp = cache
            .query(&CatalogFilter { kind: Some(ExtensionKind::Mcp), ..Default::default() })
            .await
            .unwrap();
        assert_eq!(mcp.len(), crate::hub::official_mcp::primer_entries().len());
        // Any plugin entries primed are well-formed and live in the aleph-hub slot.
        let plugins = cache
            .query(&CatalogFilter { kind: Some(ExtensionKind::Plugin), ..Default::default() })
            .await
            .unwrap();
        assert_eq!(plugins.len(), crate::hub::official_plugins::primer_entries().len());
        for p in &plugins {
            assert_eq!(p.source_id, ALEPH_HUB_ID);
            assert_eq!(p.trust_tier, crate::hub::types::TrustTier::Official);
        }
    }
```

(The existing tests mod already has `use super::*;`, `use crate::hub::cache::CatalogFilter;`, and `use crate::hub::types::ExtensionKind;`. `ALEPH_HUB_ID` is in scope via `use super::*;` re-exporting the module-top `use crate::hub::catalog_client::ALEPH_HUB_ID;`.)

- [ ] **Step 8: Run the primer + install tests — expect PASS**

Run: `cargo test -p alephcore --lib -- hub::primer hub::install::tests::plugin_marketplace_name`
Expected: PASS (existing primer tests + new `plugins_compose_without_clobbering_mcp` + install helper test).

- [ ] **Step 9: Commit**

```bash
git add src/hub/primer.rs src/hub/install.rs
git commit -m "hub: compose official plugins into primer and resolve their install marketplace"
```

---

## Self-Review

**1. Spec coverage** (spec §4 decisions → tasks):
- D1 (primer projection) → Task 1 `official_plugins.rs`. ✓
- D2 (enumerate from marketplace manifest) → Task 1 Step 1 `primer_entries` reads `.claude-plugin/marketplace.toml`. ✓
- D3 (id/name) → Task 1 `project_plugin` (`aleph-hub:<name>`, `name = entry.name`). ✓
- D4 (unified primer compose) → Task 2 Step 6. ✓
- D5 (install-path fix) → Task 2 Steps 1–5 `plugin_marketplace_name`. ✓
- D6/D7/D8 (no reconcile/migration/RPC/Panel) → no tasks (deliberately untouched). ✓
- install_spec form (GitDir, requires_config=false) → Task 1 `project_plugin` + test assertion. ✓
- §8 tests (projection / tolerates-absent / pure-fn / no-clobber) → Task 1 Steps + Task 2 Steps 1,7. ✓

**2. Placeholder scan:** No TBD/TODO; every code step shows complete code; test bodies are concrete. ✓

**3. Type consistency:** `primer_entries() -> Vec<ExtensionEntry>` consumed by Task 2 Step 6 `.extend(...)` ✓. `plugin_marketplace_name(&str) -> Option<&str>` matches `install_to_scope(.., Option<&str>, ..)` second param ✓. `ALEPH_HUB_ID`/`BUILTIN_MARKETPLACE_NAME` are `&str` ✓. `MarketplacePluginEntry` field access (`name`/`source`/`description`/`version`) matches spec §2 ✓.

**Known impl note:** if `BUNDLED_PLUGINS.get_file(".claude-plugin/marketplace.toml")` returns `None` for a populated bundle (an `include_dir` dot-dir quirk), fall back to `BUNDLED_PLUGINS.get_dir(".claude-plugin").and_then(|d| d.get_file(".claude-plugin/marketplace.toml"))` or traverse `.files()`. The projection tests use a synthetic string and the `tolerates_absent_bundle` test accepts `[]`, so this never blocks the test gate — verify against the real bundle at integration if available.
