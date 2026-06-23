# MCP → Aleph Hub Convergence Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the Aleph Hub the single MCP discovery + install + installed-status surface: cold-start–prime the official presets into the `aleph-hub` cache slot, install official MCP through the existing Hub pipeline, migrate orphaned old-path installs, and retire the parallel `mcp.list_presets`/`mcp.install_preset` surface.

**Architecture:** A cold-start *primer* projects `src/mcp/presets/catalog.json` into `ExtensionEntry`s written to the existing `aleph-hub` source slot **iff the cache is empty** (the async remote fetch overwrites it later — no peer source, no local dedup; honors `2026-06-20-aleph-hub-single-source-design.md`). Official MCP then installs through the already-wired `extensions.install` → `run_install` → `mcp_config_from_spec` (vault) → `add_server` path; installed-status reconciles unchanged because primer entry id `aleph-hub:<slug>` → `mcp_server_id` → `local:mcp:aleph-hub_<slug>` already matches (`catalog.rs:71`). A boot migration removes servers persisted under the retired raw-slug ids so the user re-installs from the Hub.

**Tech Stack:** Rust (alephcore, tokio, serde, rusqlite), Leptos/WASM panel.

## Global Constraints

- **MSRV 1.95**; toolchain pinned by `rust-toolchain.toml` (1.96.0). No `cargo +<ver>`.
- **No new crates.** `which` is already an alephcore dependency (used under `src/.../browser`); confirm with `grep -n '^which' Cargo.toml` before Task 8. No second async runtime; tokio only. No non-serde serialization. No platform-API crates in `src`.
- **Single source slot:** the primer writes to `source_id = "aleph-hub"` (`alephcore::hub::catalog_client::ALEPH_HUB_ID`). No new source, no local dedup.
- **Canonical id:** official MCP entry id = `format!("aleph-hub:{}", preset.id)`. `mcp_server_id` turns `:`/`/` into `_`, so reconcile (`local:mcp:aleph-hub_<slug>`) is unchanged.
- **Cargo discipline (from CLAUDE.md):** default to `cargo test -p alephcore --lib <filter>` for Rust units; **at most one** `cargo check -p alephcore --lib` before merge; panel verified with `cargo check -p aleph-panel --target wasm32-unknown-unknown`. No full `cargo test`.
- **Commits:** English, `<scope>: <description>`. Attribution disabled globally — **no `Co-Authored-By` trailer**.
- **Redlines:** R3/R7/R10 + single-source. Net deletion expected. Gateway handlers stay pure I/O (R4); do not touch auth/origin logic.

---

### Task 1: `CatalogCache::count_source`

The primer needs to know whether the `aleph-hub` slot is empty.

**Files:**
- Modify: `src/hub/cache.rs` (add a free function next to `clear_source` ~line 106, and a method on `CatalogCache` ~line 144)
- Test: `src/hub/cache.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Produces: `pub fn count_source(conn: &Connection, source_id: &str) -> rusqlite::Result<usize>` and `CatalogCache::count_source(&self, source_id: &str) -> rusqlite::Result<usize>` (async).

- [ ] **Step 1: Write the failing test** — add to `mod tests` in `src/hub/cache.rs`:

```rust
    #[test]
    fn count_source_counts_only_that_source() {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        let mut a = entry("a", ExtensionCategory::Developer, "Alpha");
        a.source_id = "aleph-hub".into();
        upsert_entry(&conn, &a).unwrap();
        upsert_entry(&conn, &entry("b", ExtensionCategory::Data, "Beta")).unwrap(); // mcp-official
        assert_eq!(count_source(&conn, "aleph-hub").unwrap(), 1);
        assert_eq!(count_source(&conn, "nope").unwrap(), 0);
    }
```

- [ ] **Step 2: Run it, expect failure**

Run: `cargo test -p alephcore --lib hub::cache::tests::count_source_counts_only_that_source`
Expected: FAIL — `cannot find function 'count_source'`.

- [ ] **Step 3: Implement.** Add the free function after `clear_source` (after line 111):

```rust
pub fn count_source(conn: &Connection, source_id: &str) -> rusqlite::Result<usize> {
    conn.query_row(
        "SELECT COUNT(*) FROM catalog WHERE source_id = ?1",
        params![source_id],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n as usize)
}
```

Add the method inside `impl CatalogCache` (after `replace_source`, ~line 155):

```rust
    /// Number of cached rows for a source. Used by the cold-start primer.
    pub async fn count_source(&self, source_id: &str) -> rusqlite::Result<usize> {
        let guard = self.conn.lock().await;
        count_source(&guard, source_id)
    }
```

- [ ] **Step 4: Run it, expect pass**

Run: `cargo test -p alephcore --lib hub::cache::tests::count_source_counts_only_that_source`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/hub/cache.rs
git commit -m "hub: add CatalogCache::count_source for cold-start primer"
```

---

### Task 2: Official-MCP primer projection (`src/hub/official_mcp.rs`)

Project `presets::catalog()` into `ExtensionEntry`s with an **env-based** `install_spec` (the Hub install path does not interpolate `<ENV_KEY>` placeholders, so a transport whose url/args contain `<` is not projectable — amap's `http` url is skipped in favor of its `stdio` transport).

**Files:**
- Create: `src/hub/official_mcp.rs`
- Modify: `src/hub/mod.rs` (add `pub mod official_mcp;`)
- Test: in `src/hub/official_mcp.rs`

**Interfaces:**
- Consumes: `crate::mcp::presets::{catalog, McpPreset, PresetCategory, PresetEnvVar, PresetTransport}`; `crate::mcp::manager::McpTransportType`; `crate::hub::types::{EnvDecl, ExtensionCategory, ExtensionEntry, ExtensionKind, InstallSpec, McpTransport, TrustTier}`; `crate::hub::catalog_client::ALEPH_HUB_ID`.
- Produces: `pub fn primer_entries() -> Vec<ExtensionEntry>`.

- [ ] **Step 1: Register the module.** In `src/hub/mod.rs` add (alphabetically near the other `pub mod` lines):

```rust
pub mod official_mcp;
```

- [ ] **Step 2: Write the failing test.** Create `src/hub/official_mcp.rs` with ONLY the test module first:

```rust
//! Cold-start primer + legacy migration for official MCP presets.
//!
//! Projects the in-binary `src/mcp/presets/catalog.json` into `ExtensionEntry`s
//! under the `aleph-hub` source slot so official MCP is browsable/installable
//! offline and before the remote catalog is first fetched. The remote fetch
//! later overwrites the slot wholesale (no peer source, no local dedup).

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub::types::{ExtensionKind, InstallSpec, TrustTier};

    fn by_id(entries: &[crate::hub::types::ExtensionEntry], id: &str) -> crate::hub::types::ExtensionEntry {
        entries.iter().find(|e| e.id == id).cloned().unwrap_or_else(|| panic!("missing {id}"))
    }

    #[test]
    fn primer_ids_are_aleph_hub_prefixed_and_official() {
        let e = primer_entries();
        let ctx = by_id(&e, "aleph-hub:context7");
        assert_eq!(ctx.kind, ExtensionKind::Mcp);
        assert_eq!(ctx.trust_tier, TrustTier::Official);
        assert_eq!(ctx.source_id, "aleph-hub");
    }

    #[test]
    fn context7_projects_to_keyless_remote() {
        let e = primer_entries();
        let ctx = by_id(&e, "aleph-hub:context7");
        match ctx.install_spec.unwrap() {
            InstallSpec::McpRemote { url, .. } => assert_eq!(url, "https://mcp.context7.com/mcp"),
            other => panic!("expected McpRemote, got {other:?}"),
        }
        assert!(!ctx.requires_config);
    }

    #[test]
    fn amap_skips_key_interpolated_http_for_stdio_env() {
        let e = primer_entries();
        let amap = by_id(&e, "aleph-hub:amap");
        match amap.install_spec.unwrap() {
            InstallSpec::McpStdio { command, env, .. } => {
                assert_eq!(command, "npx");
                assert!(env.iter().any(|d| d.name == "AMAP_MAPS_API_KEY" && d.required && d.secret));
            }
            other => panic!("expected McpStdio (http url has <KEY>), got {other:?}"),
        }
        assert!(amap.requires_config);
    }

    #[test]
    fn veimagex_carries_all_four_env_decls() {
        let e = primer_entries();
        let v = by_id(&e, "aleph-hub:volcengine-veimagex");
        match v.install_spec.unwrap() {
            InstallSpec::McpStdio { command, env, .. } => {
                assert_eq!(command, "uvx");
                assert_eq!(env.len(), 4);
            }
            other => panic!("expected McpStdio, got {other:?}"),
        }
    }
}
```

- [ ] **Step 3: Run it, expect failure**

Run: `cargo test -p alephcore --lib hub::official_mcp`
Expected: FAIL — `cannot find function 'primer_entries'`.

- [ ] **Step 4: Implement.** Insert above the test module in `src/hub/official_mcp.rs`:

```rust
use crate::hub::catalog_client::ALEPH_HUB_ID;
use crate::hub::types::{
    EnvDecl, ExtensionCategory, ExtensionEntry, ExtensionKind, InstallSpec, McpTransport, TrustTier,
};
use crate::mcp::manager::McpTransportType;
use crate::mcp::presets::{self, McpPreset, PresetCategory, PresetEnvVar, PresetTransport};

fn map_category(c: PresetCategory) -> ExtensionCategory {
    match c {
        PresetCategory::Developer => ExtensionCategory::Developer,
        PresetCategory::Daily => ExtensionCategory::Utilities,
        PresetCategory::ModelProvider => ExtensionCategory::Design,
    }
}

fn map_env(ev: &PresetEnvVar) -> EnvDecl {
    let description = if ev.description.is_empty() { ev.label.clone() } else { ev.description.clone() };
    EnvDecl {
        name: ev.key.clone(),
        description: Some(description),
        required: ev.required,
        secret: ev.secret,
        default: ev.default.clone(),
        placeholder: None,
    }
}

/// A transport is projectable iff it carries no `<ENV_KEY>` placeholder — the
/// Hub install path injects keys via env/headers, never by string interpolation.
fn is_projectable(t: &PresetTransport) -> bool {
    let clean = |s: &str| !s.contains('<');
    match t.kind {
        McpTransportType::Stdio => t.args.iter().all(|a| clean(a)),
        McpTransportType::Http | McpTransportType::Sse => t.url.as_deref().map(clean).unwrap_or(false),
    }
}

fn map_install_spec(p: &McpPreset) -> Option<InstallSpec> {
    let t = p.transports.iter().find(|t| is_projectable(t))?;
    let env: Vec<EnvDecl> = p.required_env.iter().map(map_env).collect();
    Some(match t.kind {
        McpTransportType::Stdio => InstallSpec::McpStdio {
            command: t.command.clone().unwrap_or_default(),
            args: t.args.clone(),
            env,
        },
        McpTransportType::Http => InstallSpec::McpRemote {
            url: t.url.clone().unwrap_or_default(),
            transport: McpTransport::StreamableHttp,
            headers: vec![],
        },
        McpTransportType::Sse => InstallSpec::McpRemote {
            url: t.url.clone().unwrap_or_default(),
            transport: McpTransport::Sse,
            headers: vec![],
        },
    })
}

fn map_entry(p: &McpPreset) -> Option<ExtensionEntry> {
    let spec = map_install_spec(p)?;
    Some(ExtensionEntry {
        id: format!("{ALEPH_HUB_ID}:{}", p.id),
        kind: ExtensionKind::Mcp,
        category: map_category(p.category),
        name: p.name.clone(),
        description: p.description.clone(),
        author: Some(p.vendor.clone()),
        icon: None,
        tags: p.tags.clone(),
        version: None,
        source_id: ALEPH_HUB_ID.to_string(),
        repo_url: None,
        trust_tier: if p.official { TrustTier::Official } else { TrustTier::Community },
        requires_config: spec.requires_config(),
        config_schema: None,
        installed: false,
        enabled: false,
        update_available: false,
        via: Some(ALEPH_HUB_ID.to_string()),
        install_spec: Some(spec),
    })
}

/// Project the in-binary official MCP preset catalog into Hub catalog entries.
pub fn primer_entries() -> Vec<ExtensionEntry> {
    presets::catalog().iter().filter_map(map_entry).collect()
}
```

- [ ] **Step 5: Run it, expect pass**

Run: `cargo test -p alephcore --lib hub::official_mcp`
Expected: PASS (4 tests).

- [ ] **Step 6: Commit**

```bash
git add src/hub/official_mcp.rs src/hub/mod.rs
git commit -m "hub: project official MCP presets into aleph-hub catalog entries"
```

---

### Task 3: Cold-start primer + boot wiring + reconcile guard

Prime the `aleph-hub` slot when empty, and assert the official-slug ids reconcile.

**Files:**
- Modify: `src/hub/official_mcp.rs` (add `prime_official_mcp_if_empty`)
- Modify: `src/bin/aleph-server/commands/start/mod.rs` (in the `early_catalog_cache` open block, ~lines 795–831, the `Ok(cache) =>` arm)
- Test: `src/hub/official_mcp.rs` + a reconcile guard in `src/gateway/handlers/extensions/catalog.rs` tests

**Interfaces:**
- Produces: `pub async fn prime_official_mcp_if_empty(cache: &crate::hub::cache::CatalogCache)`.

- [ ] **Step 1: Write the failing test** — append to `mod tests` in `src/hub/official_mcp.rs`:

```rust
    #[tokio::test]
    async fn primes_when_empty_then_is_noop_when_populated() {
        use crate::hub::cache::{CatalogCache, CatalogFilter};
        let cache = CatalogCache::open_in_memory().unwrap();
        prime_official_mcp_if_empty(&cache).await;
        let after = cache
            .query(&CatalogFilter { source_id: Some("aleph-hub".into()), ..Default::default() })
            .await
            .unwrap();
        assert!(after.iter().any(|e| e.id == "aleph-hub:context7"));
        let count = after.len();
        // Second call is a no-op (slot already non-empty).
        prime_official_mcp_if_empty(&cache).await;
        let again = cache
            .query(&CatalogFilter { source_id: Some("aleph-hub".into()), ..Default::default() })
            .await
            .unwrap();
        assert_eq!(again.len(), count);
    }
```

- [ ] **Step 2: Run it, expect failure**

Run: `cargo test -p alephcore --lib hub::official_mcp::tests::primes_when_empty`
Expected: FAIL — `cannot find function 'prime_official_mcp_if_empty'`.

- [ ] **Step 3: Implement** in `src/hub/official_mcp.rs` (after `primer_entries`):

```rust
/// Cold-start primer: if the `aleph-hub` slot is empty (never fetched), fill it
/// with the official preset projection so official MCP is available offline.
/// The async remote fetch later `replace_source`s the slot wholesale.
pub async fn prime_official_mcp_if_empty(cache: &crate::hub::cache::CatalogCache) {
    match cache.count_source(ALEPH_HUB_ID).await {
        Ok(0) => {
            let entries = primer_entries();
            match cache.replace_source(ALEPH_HUB_ID, &entries).await {
                Ok(()) => tracing::info!(count = entries.len(), "primed official MCP catalog (cold start)"),
                Err(e) => tracing::warn!(error = %e, "failed to prime official MCP catalog"),
            }
        }
        Ok(_) => {}
        Err(e) => tracing::warn!(error = %e, "count_source failed; skipping official MCP primer"),
    }
}
```

- [ ] **Step 4: Run it, expect pass**

Run: `cargo test -p alephcore --lib hub::official_mcp::tests::primes_when_empty`
Expected: PASS.

- [ ] **Step 5: Add the reconcile guard** — append to `mod tests` in `src/gateway/handlers/extensions/catalog.rs`:

```rust
    #[test]
    fn official_primer_slug_reconciles_against_live_server() {
        // primer id "aleph-hub:volcengine-veimagex" -> server id "aleph-hub_volcengine-veimagex"
        let mut catalog = vec![catalog_entry(
            "aleph-hub:volcengine-veimagex",
            ExtensionKind::Mcp,
            "veImageX",
        )];
        let installed = vec![installed_entry(
            "local:mcp:aleph-hub_volcengine-veimagex",
            ExtensionKind::Mcp,
            "veImageX",
            true,
        )];
        mark_installed(&mut catalog, &installed);
        assert!(catalog[0].installed);
    }
```

Run: `cargo test -p alephcore --lib gateway::handlers::extensions::catalog::tests::official_primer_slug_reconciles`
Expected: PASS (confirms the canonical-id chain needs no change).

- [ ] **Step 6: Wire into boot.** In `src/bin/aleph-server/commands/start/mod.rs`, inside the `early_catalog_cache` block, in the `Ok(cache) =>` arm, **after** `init_schema` has run (i.e. after `CatalogCache::open` succeeds) and **before** the tuple `(Some(std::sync::Arc::new(cache)), Some(configs))` is returned, insert:

```rust
                // Cold-start: seed official MCP into the aleph-hub slot if empty.
                alephcore::hub::official_mcp::prime_official_mcp_if_empty(&cache).await;
```

(Place it immediately before `(Some(std::sync::Arc::new(cache)), Some(configs))`. The block is already `async` — `app_config.read().await` runs just above.)

- [ ] **Step 7: Compile-check the binary touch**

Run: `cargo check -p alephcore --lib`
Expected: clean (this is the single allowed pre-merge `cargo check`).

- [ ] **Step 8: Commit**

```bash
git add src/hub/official_mcp.rs src/gateway/handlers/extensions/catalog.rs src/bin/aleph-server/commands/start/mod.rs
git commit -m "hub: cold-start prime official MCP into aleph-hub slot at boot"
```

---

### Task 4: Legacy-preset install migration (D9)

Remove MCP servers persisted under a retired raw-slug id (old `mcp.install_preset` path) so the user re-installs from the Hub. New Hub installs never use raw-slug ids, so the id alone is near-conclusive; a shape (command/transport) match is the safety belt against a coincidentally-named user server.

**Files:**
- Modify: `src/hub/official_mcp.rs` (add `is_legacy_preset_server` + `migrate_legacy_preset_servers`)
- Modify: `src/bin/aleph-server/commands/start/mod.rs` (the `if let Some(ref h) = mcp_handle { ... }` block, ~line 1324, before `register_mcp_handlers`)
- Test: `src/hub/official_mcp.rs`

**Interfaces:**
- Consumes: `crate::mcp::manager::{McpManagerConfig, McpManagerHandle, McpTransportType}`; `McpManagerHandle::list_server_configs() -> Result<Vec<McpManagerConfig>>` (async), `McpManagerHandle::remove_server(impl Into<String>) -> Result<()>` (async).
- Produces: `pub fn is_legacy_preset_server(cfg: &McpManagerConfig) -> bool`; `pub async fn migrate_legacy_preset_servers(mcp: &McpManagerHandle)`.

- [ ] **Step 1: Write the failing test** — append to `mod tests` in `src/hub/official_mcp.rs`:

```rust
    #[test]
    fn legacy_detection_matches_old_slug_and_shape() {
        use crate::mcp::manager::McpManagerConfig;
        // minimax old install: raw slug id + matching stdio command.
        let minimax = McpManagerConfig::stdio("minimax", "MiniMax", "uvx");
        assert!(is_legacy_preset_server(&minimax));
        // amap old install: raw slug id + remote (no command) matches its http transport.
        let amap = McpManagerConfig::http("amap", "高德地图", "https://mcp.amap.com/mcp?key=k");
        assert!(is_legacy_preset_server(&amap));
        // New Hub install id is never a raw slug -> not legacy.
        let hub = McpManagerConfig::stdio("aleph-hub_minimax", "MiniMax", "uvx");
        assert!(!is_legacy_preset_server(&hub));
        // User custom server that merely shares a slug name but a different command.
        let custom = McpManagerConfig::stdio("minimax", "My MiniMax", "/opt/custom");
        assert!(!is_legacy_preset_server(&custom));
        // Unknown id -> not legacy.
        let other = McpManagerConfig::stdio("totally-custom", "X", "node");
        assert!(!is_legacy_preset_server(&other));
    }
```

- [ ] **Step 2: Run it, expect failure**

Run: `cargo test -p alephcore --lib hub::official_mcp::tests::legacy_detection`
Expected: FAIL — `cannot find function 'is_legacy_preset_server'`.

- [ ] **Step 3: Implement** in `src/hub/official_mcp.rs` (add the manager import to the top `use` block, then the functions):

Add to imports:
```rust
use crate::mcp::manager::{McpManagerConfig, McpManagerHandle};
```

Add functions (after `prime_official_mcp_if_empty`):
```rust
/// True iff `cfg` was installed via the retired preset path: its id is a known
/// preset slug AND its launch shape matches that preset. New Hub installs use
/// `aleph-hub_<slug>` ids, so a raw-slug id never collides with a Hub install.
pub fn is_legacy_preset_server(cfg: &McpManagerConfig) -> bool {
    let Some(preset) = presets::find(&cfg.id) else {
        return false;
    };
    preset.transports.iter().any(|t| match t.kind {
        McpTransportType::Stdio => t.command == cfg.command,
        McpTransportType::Http | McpTransportType::Sse => cfg.command.is_none(),
    })
}

/// Boot migration (D9): remove servers installed via the retired preset path so
/// the user re-installs from the Hub. Warn-only; never aborts boot.
pub async fn migrate_legacy_preset_servers(mcp: &McpManagerHandle) {
    let configs = match mcp.list_server_configs().await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "preset migration: list_server_configs failed");
            return;
        }
    };
    for cfg in configs {
        if is_legacy_preset_server(&cfg) {
            let id = cfg.id.clone();
            match mcp.remove_server(id.clone()).await {
                Ok(()) => tracing::info!(%id, "removed retired-preset MCP server; re-install from Aleph Hub"),
                Err(e) => tracing::warn!(%id, error = %e, "preset migration: remove_server failed"),
            }
        }
    }
}
```

> If `McpManagerConfig` exposes `command` as a method or differently-named field, adjust `t.command == cfg.command` to compare `Option<String>` accordingly (confirm against `src/mcp/manager/types.rs`). The field is `command: Option<String>` per `mcp.rs` test `params.config.command == Some(...)`.

- [ ] **Step 4: Run it, expect pass**

Run: `cargo test -p alephcore --lib hub::official_mcp::tests::legacy_detection`
Expected: PASS.

- [ ] **Step 5: Wire into boot.** In `src/bin/aleph-server/commands/start/mod.rs`, in the block `if let Some(ref h) = mcp_handle {` (~line 1324), **before** `register_mcp_handlers(&mut server, h);` insert:

```rust
        alephcore::hub::official_mcp::migrate_legacy_preset_servers(h).await;
```

- [ ] **Step 6: Commit**

```bash
git add src/hub/official_mcp.rs src/bin/aleph-server/commands/start/mod.rs
git commit -m "hub: migrate off retired MCP preset installs at boot (D9)"
```

---

### Task 5: Remove the Settings ▸ MCP "Recommended" section (panel)

> **D6 deviation — confirm before executing.** The spec's D6 default was "re-point" the cards to `extensions.*`. The panel `ExtensionEntry` (`api/extensions.rs`) does **not** carry `install_spec`, and `EnvDecl` lacks `how_to_get_url`, so re-pointing needs backend+frontend changes plus a "how-to-get-key" UX regression. The Extensions Hub already provides full MCP discovery+install. This task **removes** the Recommended section (one discovery surface, simplest, unblocks the RPC retirement in Task 6). If you prefer re-point, stop and revisit Task 5/6.

**Files:**
- Modify: `interfaces/webchat/src/views/settings/mcp.rs` (remove `load_presets` ~65–71; the `presets` + `installing_preset` signals in `McpView`; the "Recommended Presets" UI block ~187–235; the `<Show>` that renders `InstallPresetDialog`; the `InstallPresetDialog` component ~665–819; and the now-unused imports of `McpPresetApi`/`McpPresetInfo`/`McpPresetEnvVar`/`PresetInstallOutcome`)
- Modify: `interfaces/webchat/src/api/mcp.rs` (delete `McpPresetEnvVar` ~85–101, `McpPresetInfo` ~103–123, `PresetInstallOutcome` ~125–134, `McpPresetApi` ~136–189). Keep `McpConfigApi` and `McpServerInfo`/`McpServerConfig`.

**Interfaces:**
- Consumes: nothing new.
- Produces: Settings ▸ MCP no longer references the preset RPCs.

- [ ] **Step 1: Remove the panel API types.** In `interfaces/webchat/src/api/mcp.rs`, delete the `McpPresetEnvVar`, `McpPresetInfo`, `PresetInstallOutcome`, and `McpPresetApi` items (everything from `/// One env var a preset needs;` to end of `impl McpPresetApi`). Leave the `McpConfigApi` block intact.

- [ ] **Step 2: Remove the view usages.** In `interfaces/webchat/src/views/settings/mcp.rs`:
  - Delete the `load_presets` fn (~65–71).
  - In `McpView`, delete the `presets` and `installing_preset` `RwSignal` declarations and any `load_presets(...)` call.
  - Delete the "Recommended Presets" section markup (~187–235).
  - Delete the `<Show when=move || installing_preset.get().is_some() ...>` block rendering `InstallPresetDialog`.
  - Delete the `InstallPresetDialog` component (~665–819).
  - Remove `McpPresetApi`, `McpPresetInfo`, `McpPresetEnvVar`, `PresetInstallOutcome` from the `use crate::api::mcp::{...}` import.

- [ ] **Step 3: Compile the panel**

Run: `cargo check -p aleph-panel --target wasm32-unknown-unknown`
Expected: clean — no references to the deleted symbols remain. (Fix any dangling references the compiler reports — they are the exact spots to delete.)

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/views/settings/mcp.rs interfaces/webchat/src/api/mcp.rs
git commit -m "panel: drop Settings MCP recommended section; discovery moves to Aleph Hub"
```

---

### Task 6: Retire `mcp.list_presets` / `mcp.install_preset`

With the panel no longer calling them, remove the handlers + their helpers + the registrations.

**Files:**
- Modify: `src/gateway/handlers/mcp.rs` (delete `preset_view` ~496–503, `install_plan_to_json` ~505–515, `InstallPresetParams` ~517–523, `handle_list_presets` ~525–540, `handle_install_preset` ~542–586, and the now-unused `use` of `presets`, `InstallPlan`, `check_runtime`/`RuntimeKind` **only if** no other handler in the file uses them)
- Modify: `src/bin/aleph-server/commands/start/builder/handlers/mcp.rs` (delete lines 47–49: the comment + two `reg!` lines)

**Interfaces:**
- Produces: the gateway no longer registers `mcp.list_presets` / `mcp.install_preset`.

- [ ] **Step 1: Delete the registrations.** In `src/bin/aleph-server/commands/start/builder/handlers/mcp.rs` remove:

```rust
    // Preset catalog (built-in recommended MCP servers)
    reg!("mcp.list_presets", mcp::handle_list_presets);
    reg!("mcp.install_preset", mcp::handle_install_preset);
```

- [ ] **Step 2: Delete the handlers + helpers.** In `src/gateway/handlers/mcp.rs` delete the entire `// === Preset Handlers ===` section: `preset_view`, `install_plan_to_json`, `InstallPresetParams`, `handle_list_presets`, `handle_install_preset`.

- [ ] **Step 3: Prune now-unused imports.** Build and let the compiler flag unused imports in `mcp.rs` (e.g. `presets`, `InstallPlan`, `check_runtime`, `RuntimeKind`, `RESOURCE_NOT_FOUND`). Remove exactly those the compiler reports as unused. Do not remove imports still used by surviving handlers.

Run: `cargo check -p alephcore --lib` is **not** spent here (budget: one check, used in Task 3). Instead rely on the next task's test pass + a final pre-merge check. If you must verify now, prefer `cargo build -p alephcore --lib 2>&1 | grep -E "unused|error"` mentally; otherwise proceed — Task 7 ends with the allowed check.

- [ ] **Step 4: Commit**

```bash
git add src/gateway/handlers/mcp.rs src/bin/aleph-server/commands/start/builder/handlers/mcp.rs
git commit -m "gateway: retire mcp.list_presets/install_preset (superseded by Aleph Hub)"
```

---

### Task 7: Retire the preset install engine (D7)

`plan_install` and friends now have zero consumers. Keep the catalog **data** (`catalog()`/`find()` + the `McpPreset`/`PresetTransport`/`PresetEnvVar`/`PresetCategory`/`Reachability` types + `catalog.json`) which the primer and migration use.

**Files:**
- Modify: `src/mcp/presets/mod.rs` (delete `InstallPlan` enum ~116–127; the `impl McpPreset` methods `missing_required_env`, `effective_env`, `materialize`, `plan_install` ~129–232; and the engine tests `needs_key_when_required_secret_missing`, `already_installed_when_id_present`, `amap_remote_first_substitutes_key_into_url`, `no_runtime_when_only_transport_runtime_unavailable`, `minimax_applies_default_host`, plus the `env(...)` test helper). Keep `bundled_catalog_parses_and_has_first_batch` and `amap_requires_secret_key_and_has_remote_first`.)

**Interfaces:**
- Produces: `presets` exposes only data (`catalog`, `find`, types). No install engine.

- [ ] **Step 1: Delete the engine.** Remove `InstallPlan` and the four `impl McpPreset` methods. Remove the `use std::collections::HashMap;` and `use crate::mcp::manager::{McpManagerConfig, McpTransportType};` imports **iff** unused after deletion — note `McpTransportType` is still referenced by `is_legacy_preset_server`/primer **in `official_mcp.rs`, not here**; in `presets/mod.rs` it was only used by `materialize`, so it likely becomes unused here. Let the compiler confirm.

- [ ] **Step 2: Delete the engine tests** listed above (they exercise `plan_install`). Keep the two data tests.

- [ ] **Step 3: Run the surviving preset tests**

Run: `cargo test -p alephcore --lib mcp::presets`
Expected: PASS — `bundled_catalog_parses_and_has_first_batch` + `amap_requires_secret_key_and_has_remote_first` only.

- [ ] **Step 4: Final compile check (the one allowed pre-merge check if Task 3's was long ago)**

Run: `cargo check -p alephcore --lib`
Expected: clean. Resolve any unused-import warnings introduced by Tasks 6–7.

- [ ] **Step 5: Commit**

```bash
git add src/mcp/presets/mod.rs
git commit -m "mcp: retire preset install engine; catalog.json is now Hub seed data (R10)"
```

---

### Task 8: NoRuntime pre-check in `run_install` (D8)

Fail fast (clear message) instead of persisting a server whose command isn't on PATH. Lowest-priority task — the convergence works without it; drop if `which` is unavailable.

**Files:**
- Modify: `src/hub/install.rs` (`run_install`, the `McpStdio | McpRemote` arm ~143–156; add a small `command_available` helper + test)
- Test: `src/hub/install.rs`

**Interfaces:**
- Consumes: `which::which` (confirm `which` is in `Cargo.toml`).
- Produces: `fn command_available(command: &str) -> bool`.

- [ ] **Step 1: Confirm the dependency**

Run: `grep -n '^which' Cargo.toml`
Expected: a `which = "..."` line. If absent, **skip this task** (note it in the PR; the rest of the plan stands).

- [ ] **Step 2: Write the failing test** — append to `mod tests` in `src/hub/install.rs`:

```rust
    #[test]
    fn absent_command_is_unavailable() {
        assert!(!command_available("definitely-not-a-real-command-xyz-123"));
    }
```

- [ ] **Step 3: Run it, expect failure**

Run: `cargo test -p alephcore --lib hub::install::tests::absent_command_is_unavailable`
Expected: FAIL — `cannot find function 'command_available'`.

- [ ] **Step 4: Implement.** Add the helper near `mcp_server_id` in `src/hub/install.rs`:

```rust
/// True if `command` resolves on PATH (PATHEXT-aware via `which`). Used to
/// fail an install fast rather than persist a server that can't spawn.
fn command_available(command: &str) -> bool {
    which::which(command).is_ok()
}
```

In `run_install`, inside the `InstallSpec::McpStdio { .. } | InstallSpec::McpRemote { .. } =>` arm, **before** `let mcp = ctx.mcp...`, add:

```rust
            if let InstallSpec::McpStdio { command, .. } = spec {
                if !command_available(command) {
                    return Err(format!(
                        "required command '{command}' not found on PATH — install its runtime (e.g. node/python) and retry"
                    ));
                }
            }
```

- [ ] **Step 5: Run it, expect pass**

Run: `cargo test -p alephcore --lib hub::install::tests::absent_command_is_unavailable`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/hub/install.rs
git commit -m "hub: fail MCP install fast when stdio command is missing (D8)"
```

---

## Self-Review

**Spec coverage:** D1 (retire rogue path) → Tasks 5–6. D2 (cold-start primer, same slot) → Tasks 2–3. D3 (canonical id `aleph-hub:<slug>`) → Task 2 + reconcile guard (Task 3). D4 (install via Hub engine + vault) → already wired (`extensions.install`/`run_install`); no task needed beyond verifying — covered by the reconcile guard + existing `stdio_spec_builds_config_with_secret_refs`. D5 (no remote-trust clamp) → no clamp added (nothing to do). D6 (Settings Recommended) → Task 5 (removed, deviation flagged). D7 (retire engine) → Task 7. D8 (NoRuntime) → Task 8. D9 (migration) → Task 4. D10 (clearinghouse) → no code (install_spec already points upstream). §5 cross-repo id contract → **verification, not code** (see Open Items).

**Placeholder scan:** none — every code step carries full code; deletion tasks list exact symbols + line anchors and gate on compile.

**Type consistency:** `primer_entries`/`prime_official_mcp_if_empty`/`is_legacy_preset_server`/`migrate_legacy_preset_servers` all live in `src/hub/official_mcp.rs` and are referenced as `alephcore::hub::official_mcp::*` from boot. `ALEPH_HUB_ID` used consistently. `mcp_server_id`/`mark_installed` unchanged. `McpManagerConfig.command: Option<String>` assumption flagged for verification in Task 4.

## Open Items (carry to execution / PR)

1. **§5 cross-repo id contract:** confirm the Aleph-Hub website emits official-MCP entry ids as `aleph-hub:<catalog.json id>` (e.g. `aleph-hub:volcengine-veimagex`). If the website uses a different slug, the primer→remote handoff will split installed-status. Align before the remote catalog ships these as official. (User-owned; not in this repo.)
2. **D6 deviation (Task 5):** removed the Settings Recommended section rather than re-pointing. Confirm acceptable, or request the re-point variant.
3. **`McpManagerConfig.command` field name** (Task 4): verify against `src/mcp/manager/types.rs`.
