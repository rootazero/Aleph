# Aleph Hub Single-Source Teardown — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Collapse Aleph's local extension federation into a thin single-source consumer of one published Aleph Hub catalog, persisting install specs in the cache.

**Architecture:** Delete the `SourceProvider` trait + `ProviderRegistry` + the 3 live browse providers + `dedup`/`display`/`categorize`; promote `StaticHubProvider` into a standalone `AlephHubCatalog` HTTP client. Provenance (`via`) and the resolved `install_spec` now ride on each cached `ExtensionEntry`, written by `into_entry` from the published artifact, so install resolution is a pure cache lookup.

**Tech Stack:** Rust (alephcore lib + aleph-server bin, tokio + serde + rusqlite + reqwest), Leptos/WASM panel.

**Design spec:** `docs/superpowers/specs/2026-06-20-aleph-hub-single-source-design.md`

## Global Constraints

- Toolchain pinned `1.96.0`, MSRV `1.95` — do not bump.
- tokio-only async; serde-only serialization; NO platform-API crates in `src` (R1).
- `ALEPH_HUB_URL = "https://hub.heyaleph.com/catalog.json"` — single named const; remove the stale `hub.aleph.computer`.
- Keep unchanged: the `extensions.*` RPC namespace, the `src/hub` module name, and the `source_label` JSON **wire key** (panel contract). Only its *value source* changes (`display::source_label(source_id)` → `e.via`).
- Commit messages: `hub: <description>` (English).
- Single-branch: commit directly on `main`.
- **Cargo frugality (project rule + memory):** the build is memory-heavy. Verify with `cargo check` scoped to `-p alephcore --lib` (lib-only tasks) or `-p alephcore --bin aleph-server` (lib+bin task); panel via `-p aleph-panel --target wasm32-unknown-unknown`. Run NEW unit tests selectively (`cargo test -p alephcore --lib <name>`), never the full suite (pre-existing broken `tests/cancellation_chain.rs`). Do NOT run `just wasm`.
- `ExtensionEntry` derives `PartialEq, Clone, Serialize, Deserialize` — the new `install_spec: Option<InstallSpec>` requires `InstallSpec` to derive `Clone + PartialEq` (verify in Task 1; it already does, used in existing `assert_eq!` tests).

**Task order is green-at-each-commit.** Task 4 deliberately bundles the gateway-handler signature change (lib) with its bin call sites because they straddle the lib/bin boundary and would not compile separately.

---

### Task 1: Add `via` + `install_spec` to the entry types and fix every struct literal

**Files:**
- Modify: `src/hub/types.rs` (ExtensionEntry def + sample_entry helper)
- Modify: `src/hub/hub_catalog.rs` (HubCatalogEntry +via, into_entry, doc comment)
- Modify (test helpers, +`via: None, install_spec: None`): `src/hub/cache.rs`, `src/hub/dedup.rs`, `src/hub/trust.rs`, `src/hub/reconcile.rs`, `src/hub/provider/mod.rs`, `src/hub/provider/mcp_registry.rs`, `src/hub/provider/docker_mcp.rs`, `src/hub/provider/marketplace.rs`, `src/builtin_tools/hub/resolve_spec.rs`

**Interfaces:**
- Produces: `ExtensionEntry { …, via: Option<String>, install_spec: Option<InstallSpec> }`; `HubCatalogEntry.via: Option<String>`; `HubCatalogEntry::into_entry(&self, hub_id) -> ExtensionEntry` populating both.

- [ ] **Step 1: Verify `InstallSpec` derives `Clone + PartialEq`**

Run: `rg "enum InstallSpec" -A1 src/hub/types.rs` and check the `#[derive(...)]` above it includes `Clone` and `PartialEq`. If missing, add them (it is referenced by `assert_eq!` in existing tests, so it should already derive both).

- [ ] **Step 2: Add the two fields to `ExtensionEntry`** (`src/hub/types.rs`, end of struct, before closing `}`)

```rust
    #[serde(default)]
    pub update_available: bool,
    /// Upstream provenance label (e.g. "clawhub", "github:owner"); filled from
    /// the published catalog. None for local/installed entries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
    /// Resolved install spec carried by the catalog entry; None for local
    /// entries. Install resolution is a pure cache lookup of this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_spec: Option<InstallSpec>,
}
```

- [ ] **Step 3: Add the failing test** (`src/hub/types.rs`, in `#[cfg(test)] mod tests`)

```rust
    #[test]
    fn entry_carries_via_and_install_spec() {
        let mut e = sample_entry();
        e.via = Some("aleph-hub".into());
        e.install_spec = Some(InstallSpec::OciImage { image: "x@sha256:abc".into() });
        let j = serde_json::to_value(&e).unwrap();
        assert_eq!(j["via"], "aleph-hub");
        assert!(j["install_spec"].is_object());
        let back: ExtensionEntry = serde_json::from_value(j).unwrap();
        assert_eq!(back, e);
    }
```

- [ ] **Step 4: Run it — expect FAIL** (missing fields)

Run: `cargo test -p alephcore --lib entry_carries_via_and_install_spec`
Expected: compile error — `sample_entry()` literal is missing `via`/`install_spec`.

- [ ] **Step 5: Fix `sample_entry()`** (`src/hub/types.rs`) — append to its literal:

```rust
            update_available: false,
            via: None,
            install_spec: None,
        }
    }
```

- [ ] **Step 6: Add `via` to `HubCatalogEntry`** (`src/hub/hub_catalog.rs`, after `install_spec: InstallSpec,`)

```rust
    pub install_spec: InstallSpec,
    /// Upstream provenance label set by the publishing hub. Additive/back-compat.
    #[serde(default)]
    pub via: Option<String>,
}
```

- [ ] **Step 7: Populate both fields in `into_entry`** (`src/hub/hub_catalog.rs`) — append to the `ExtensionEntry { … }` literal:

```rust
            installed: false,
            enabled: false,
            update_available: false,
            via: self.via.clone().or_else(|| Some(hub_id.to_string())),
            install_spec: Some(self.install_spec.clone()),
        }
    }
}
```

- [ ] **Step 8: Update the `hub_catalog.rs` module doc** (line ~6) to point at the new spec:

```rust
//! the contract with the Aleph-Hub publisher. See
//! docs/superpowers/specs/2026-06-20-aleph-hub-single-source-design.md
```

- [ ] **Step 9: Fix the remaining struct literals** — add `via: None, install_spec: None,` (just before each literal's closing `}`) at each site below. These compile now and (for the `provider/*` files) are deleted in Task 5; `None` is sufficient.

```
src/hub/cache.rs           — entry() test helper
src/hub/dedup.rs           — e() / dummy_entry() test helper
src/hub/trust.rs           — mcp_entry() test helper
src/hub/reconcile.rs       — base_entry()        (local installed items: via: None)
src/hub/provider/mod.rs    — entry() test helper
src/hub/provider/mcp_registry.rs   — server_to_extension()
src/hub/provider/docker_mcp.rs     — docker_server_to_extension()
src/hub/provider/marketplace.rs    — plugin entry constructor
src/builtin_tools/hub/resolve_spec.rs — sample_entry()
```

- [ ] **Step 10: Run the test + cache test — expect PASS**

Run: `cargo test -p alephcore --lib entry_carries_via_and_install_spec`
Expected: PASS.

- [ ] **Step 11: Verify lib compiles**

Run: `cargo check -p alephcore --lib`
Expected: no errors (warnings about unused `via`/`install_spec` are fine).

- [ ] **Step 12: Commit**

```bash
git add -A
git commit -m "hub: add via + install_spec to ExtensionEntry/HubCatalogEntry"
```

---

### Task 2: Create the standalone `AlephHubCatalog` client

**Files:**
- Create: `src/hub/catalog_client.rs`
- Modify: `src/hub/mod.rs` (add `pub mod catalog_client;`)

**Interfaces:**
- Consumes: `HubCatalogArtifact`, `SUPPORTED_SCHEMA_VERSION` (pub in `hub_catalog.rs`); `scan_for_injection` (pub in `trust.rs`); `CatalogCache::replace_source` (`async`, `Result<(), _>`).
- Produces: `AlephHubCatalog::{new, fetch, sync_into}`; `SyncReport { synced: usize, failed: Vec<String> }`; `CatalogError`; consts `ALEPH_HUB_ID/NAME/URL`. `sync_into` returns `SyncReport` **directly** (no `Result`): fetch/cache failure → `synced: 0, failed: vec![..]` (keeps last-good cache).

- [ ] **Step 1: Write the new file** `src/hub/catalog_client.rs`

```rust
//! Standalone `AlephHubCatalog` client: HTTP fetch → schema-version check →
//! injection scan → `into_entry` normalization → cache sync. No SourceProvider
//! trait, no in-memory spec map — install resolution is a pure cache lookup of
//! `ExtensionEntry.install_spec`.

use std::fmt;

use crate::hub::cache::CatalogCache;
use crate::hub::hub_catalog::{HubCatalogArtifact, SUPPORTED_SCHEMA_VERSION};
use crate::hub::trust::scan_for_injection;
use crate::hub::types::{ExtensionEntry, TrustTier};

/// Built-in official Aleph Hub source.
pub const ALEPH_HUB_ID: &str = "aleph-hub";
pub const ALEPH_HUB_NAME: &str = "Aleph Hub";
pub const ALEPH_HUB_URL: &str = "https://hub.heyaleph.com/catalog.json";

#[derive(Debug, Clone)]
pub enum CatalogError {
    Network(String),
    Parse(String),
    Schema(String),
    Other(String),
}

impl fmt::Display for CatalogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CatalogError::Network(s) => write!(f, "network: {s}"),
            CatalogError::Parse(s) => write!(f, "parse: {s}"),
            CatalogError::Schema(s) => write!(f, "schema: {s}"),
            CatalogError::Other(s) => write!(f, "{s}"),
        }
    }
}
impl std::error::Error for CatalogError {}

/// Result of one sync into the cache.
#[derive(Debug, Clone)]
pub struct SyncReport {
    pub synced: usize,
    pub failed: Vec<String>,
}

/// Thin, stateless client for the single published Aleph Hub catalog artifact.
#[derive(Clone)]
pub struct AlephHubCatalog {
    id: String,
    #[allow(dead_code)]
    name: String,
    artifact_url: String,
    #[allow(dead_code)]
    trust_tier: TrustTier,
    http: reqwest::Client,
}

impl AlephHubCatalog {
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        artifact_url: impl Into<String>,
        trust_tier: TrustTier,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            artifact_url: artifact_url.into(),
            trust_tier,
            http: reqwest::Client::new(),
        }
    }

    /// Parse + normalize an artifact body (no network) — schema check, injection
    /// scan (warn-only), then `into_entry`.
    fn ingest(&self, body: &str) -> Result<Vec<ExtensionEntry>, CatalogError> {
        let art: HubCatalogArtifact =
            serde_json::from_str(body).map_err(|e| CatalogError::Parse(e.to_string()))?;
        if art.manifest.schema_version > SUPPORTED_SCHEMA_VERSION {
            return Err(CatalogError::Schema(format!(
                "artifact schema_version {} > supported {}",
                art.manifest.schema_version, SUPPORTED_SCHEMA_VERSION
            )));
        }
        let mut out = Vec::with_capacity(art.entries.len());
        for he in &art.entries {
            let findings = scan_for_injection(&format!("{} {}", he.name, he.description));
            if !findings.is_empty() {
                tracing::warn!(hub = %self.id, id = %he.id, ?findings, "hub entry injection findings");
            }
            out.push(he.into_entry(&art.manifest.hub_id));
        }
        Ok(out)
    }

    /// Fetch the artifact over HTTP and normalize it.
    pub async fn fetch(&self) -> Result<Vec<ExtensionEntry>, CatalogError> {
        let resp = self
            .http
            .get(&self.artifact_url)
            .timeout(std::time::Duration::from_secs(30))
            .send()
            .await
            .map_err(|e| CatalogError::Network(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(CatalogError::Network(format!("HTTP {}", resp.status())));
        }
        let body = resp
            .text()
            .await
            .map_err(|e| CatalogError::Network(e.to_string()))?;
        self.ingest(&body)
    }

    /// Fetch + atomically replace this source's cache slice. Never errors out:
    /// a fetch/cache failure yields `synced: 0` and keeps the last-good cache.
    pub async fn sync_into(&self, cache: &CatalogCache) -> SyncReport {
        match self.fetch().await {
            Ok(entries) => {
                let synced = entries.len();
                match cache.replace_source(&self.id, &entries).await {
                    Ok(()) => SyncReport { synced, failed: Vec::new() },
                    Err(e) => SyncReport { synced: 0, failed: vec![format!("cache write: {e}")] },
                }
            }
            Err(e) => SyncReport { synced: 0, failed: vec![e.to_string()] },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub::types::{ExtensionCategory, ExtensionKind, InstallSpec};

    const FIXTURE: &str = r#"{"manifest":{"schema_version":1,"hub_id":"aleph-hub","name":"Aleph Hub"},
      "entries":[{"id":"aleph-hub:acme/foo","kind":"mcp","category":"developer","name":"Foo",
      "description":"d","repo_url":"https://github.com/acme/foo","trust_tier":"verified",
      "install_spec":{"type":"mcp_stdio","command":"npx","args":["@acme/foo"],"env":[]},
      "via":"clawhub"}]}"#;

    #[test]
    fn ingest_populates_via_and_install_spec() {
        let c = AlephHubCatalog::new(ALEPH_HUB_ID, ALEPH_HUB_NAME, "http://unused", TrustTier::Verified);
        let entries = c.ingest(FIXTURE).unwrap();
        assert_eq!(entries.len(), 1);
        let e = &entries[0];
        assert_eq!(e.source_id, "aleph-hub");
        assert_eq!(e.kind, ExtensionKind::Mcp);
        assert_eq!(e.category, ExtensionCategory::Developer);
        assert_eq!(e.via.as_deref(), Some("clawhub")); // wire `via` wins
        assert!(matches!(e.install_spec, Some(InstallSpec::McpStdio { .. })));
    }

    #[test]
    fn ingest_falls_back_to_hub_id_when_via_absent() {
        let body = r#"{"manifest":{"schema_version":1,"hub_id":"aleph-hub","name":"Aleph Hub"},
          "entries":[{"id":"aleph-hub:x","kind":"mcp","category":"other","name":"X","description":"d",
          "repo_url":"https://github.com/x/x","trust_tier":"verified",
          "install_spec":{"type":"mcp_stdio","command":"c","args":[],"env":[]}}]}"#;
        let c = AlephHubCatalog::new(ALEPH_HUB_ID, ALEPH_HUB_NAME, "http://unused", TrustTier::Verified);
        let e = &c.ingest(body).unwrap()[0];
        assert_eq!(e.via.as_deref(), Some("aleph-hub")); // fallback to hub_id
    }

    #[test]
    fn ingest_rejects_future_schema() {
        let body = r#"{"manifest":{"schema_version":999,"hub_id":"h","name":"H"},"entries":[]}"#;
        let c = AlephHubCatalog::new("h", "H", "http://unused", TrustTier::Community);
        assert!(matches!(c.ingest(body), Err(CatalogError::Schema(_))));
    }

    #[test]
    fn constants_are_pinned() {
        assert_eq!(ALEPH_HUB_URL, "https://hub.heyaleph.com/catalog.json");
    }
}
```

> Before finalizing, open `src/hub/hub_catalog.rs` and confirm the exact field names used in the FIXTURE match the `Deserialize` (`schema_version`, `hub_id`, `name` on the manifest; `install_spec` tag `type`/variant names on `InstallSpec`). Adjust the JSON fixtures to the real serde representation if they differ.

- [ ] **Step 2: Register the module** (`src/hub/mod.rs`) — add alongside the others (full rewrite of mod list happens in Task 5; here just add the line):

```rust
pub mod cache;
pub mod catalog_client;
pub mod categorize;
```

- [ ] **Step 3: Run the new tests — expect PASS**

Run: `cargo test -p alephcore --lib catalog_client::`
Expected: 4 tests PASS. (If a fixture field name is wrong, fix the JSON, not the code.)

- [ ] **Step 4: Verify lib compiles**

Run: `cargo check -p alephcore --lib`

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "hub: add standalone AlephHubCatalog client (catalog_client.rs)"
```

---

### Task 3: Rewire the three hub tools off the registry onto cache + AlephHubCatalog

**Files:**
- Modify: `src/builtin_tools/hub/catalog_sync.rs`
- Modify: `src/builtin_tools/hub/resolve_spec.rs`
- Modify: `src/builtin_tools/hub/install_run.rs`

**Interfaces:**
- Consumes: `AlephHubCatalog`, `ALEPH_HUB_*` (Task 2); `ExtensionEntry.install_spec` (Task 1).
- Produces: `HubCatalogSyncOutput { synced: usize, failed: Vec<String> }`.

- [ ] **Step 1: `catalog_sync.rs` — swap imports + call() + output struct**

Replace the registry imports:
```rust
use crate::hub::catalog_client::{AlephHubCatalog, ALEPH_HUB_ID, ALEPH_HUB_NAME, ALEPH_HUB_URL};
use crate::hub::types::TrustTier;
```
Replace the `call` body:
```rust
    async fn call(&self, _args: Self::Args) -> Result<Self::Output> {
        let hub = AlephHubCatalog::new(ALEPH_HUB_ID, ALEPH_HUB_NAME, ALEPH_HUB_URL, TrustTier::Verified);
        let report = hub.sync_into(&self.cache).await;
        Ok(HubCatalogSyncOutput { synced: report.synced, failed: report.failed })
    }
```
Replace the output struct:
```rust
#[derive(Debug, Clone, Serialize)]
pub struct HubCatalogSyncOutput {
    pub synced: usize,
    pub failed: Vec<String>,
}
```
Remove any now-unused `from_report` helper + its test; delete the `marketplaces` field if `call()` no longer reads it (the single catalog needs no marketplace configs). Keep `cache`.

- [ ] **Step 2: `resolve_spec.rs` — replace registry resolution with cache lookup**

Remove `use crate::hub::provider::registry_builder::build_default_registry;`. Replace the resolution block (after the `entry` is fetched from cache):
```rust
        let spec = entry.install_spec.clone().ok_or_else(|| {
            AlephError::other(format!("no install spec cached for {}", args.entry_id))
        })?;
```
Delete the `known_entry_unknown_provider_returns_error` test (provider routing is gone). Drop the `marketplaces` field if unused.

- [ ] **Step 3: `resolve_spec.rs` — add the new tests**

```rust
    #[tokio::test]
    async fn returns_cached_install_spec() {
        let cache = CatalogCache::open_in_memory().unwrap();
        let mut e = sample_entry("aleph-hub:foo", "aleph-hub");
        e.install_spec = Some(InstallSpec::McpStdio {
            command: "npx".into(), args: vec!["@t/foo".into()], env: vec![],
        });
        cache.upsert_many(&[e]).await.unwrap();
        let tool = HubResolveSpecTool { cache: Arc::new(cache) };
        let out = tool.call(HubResolveSpecArgs { entry_id: "aleph-hub:foo".into() }).await.unwrap();
        let got: InstallSpec = serde_json::from_value(out.install_spec).unwrap();
        assert!(matches!(got, InstallSpec::McpStdio { .. }));
    }

    #[tokio::test]
    async fn errors_when_no_spec_cached() {
        let cache = CatalogCache::open_in_memory().unwrap();
        cache.upsert_many(&[sample_entry("aleph-hub:bar", "aleph-hub")]).await.unwrap();
        let tool = HubResolveSpecTool { cache: Arc::new(cache) };
        let err = tool.call(HubResolveSpecArgs { entry_id: "aleph-hub:bar".into() }).await.unwrap_err();
        assert!(err.to_string().contains("no install spec cached"));
    }
```
> Adjust `HubResolveSpecTool { … }` construction to match its real fields after you drop `marketplaces`. Ensure `sample_entry()` (updated in Task 1 to `install_spec: None`) is used for the no-spec case.

- [ ] **Step 4: `install_run.rs` — replace registry resolution with cache lookup**

Remove `use crate::hub::provider::registry_builder::build_default_registry;`. Replace:
```rust
        let spec = entry.install_spec.clone().ok_or_else(|| {
            AlephError::other(format!("no install spec cached for {}", args.entry_id))
        })?;
```
**Keep** the `marketplaces` field and the `MarketplaceManager` built for `InstallContext` (GitDir plugin installs depend on it).

- [ ] **Step 5: Run the tool tests + check**

Run: `cargo test -p alephcore --lib builtin_tools::hub::`
Run: `cargo check -p alephcore --lib`

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "hub: resolve install spec from cache; drop registry from hub tools"
```

---

### Task 4: Rewire gateway handlers + bin wiring; delete `sources.*` (ATOMIC lib+bin)

> This single task spans `alephcore` (handler signatures) and the `aleph-server` bin (call sites). They MUST land together or the bin crate won't compile.

**Files:**
- Modify: `src/gateway/handlers/extensions/catalog.rs`
- Modify: `src/gateway/handlers/extensions/install.rs`
- Delete: `src/gateway/handlers/extensions/sources.rs`
- Modify: `src/gateway/handlers/extensions/mod.rs`
- Modify: `src/bin/aleph-server/commands/start/builder/handlers/extensions.rs`
- Modify: `src/bin/aleph-server/commands/start/mod.rs`

- [ ] **Step 1: `catalog.rs` — drop dedup + display, emit `source_label` from `via`**

Remove these imports:
```rust
use crate::hub::dedup::{dedup_by_priority, DEFAULT_HUB_PRIORITY};
use crate::hub::display::source_label;
```
Replace the `Ok(entries) => { … }` arm body with (no dedup, label from `via`):
```rust
        Ok(entries) => {
            let items: Vec<serde_json::Value> = entries
                .iter()
                .map(|e| {
                    let mut v = serde_json::to_value(e).unwrap_or_else(|_| json!({}));
                    if let Some(obj) = v.as_object_mut() {
                        obj.insert("source_label".into(), json!(e.via.clone().unwrap_or_default()));
                    }
                    v
                })
                .collect();
            JsonRpcResponse::success(req.id, json!({ "extensions": items }))
        }
```

- [ ] **Step 2: `install.rs` — make `resolve_spec` a pure entry lookup; drop registry params**

Remove `use crate::hub::provider::ProviderRegistry;`. Replace the helper:
```rust
fn resolve_spec(entry: &ExtensionEntry) -> Result<InstallSpec, String> {
    entry
        .install_spec
        .clone()
        .ok_or_else(|| format!("no install_spec for entry '{}'", entry.id))
}
```
> It no longer needs `async` or `cache`/`registry`. Update its call sites from `resolve_spec(&entry, &registry).await?` to `resolve_spec(&entry)?`.

Drop the `registry: Arc<ProviderRegistry>` parameter from all three handlers:
```rust
pub async fn handle_disclosure(req: JsonRpcRequest, cache: Arc<CatalogCache>) -> JsonRpcResponse {
pub async fn handle_configure(req: JsonRpcRequest, cache: Arc<CatalogCache>) -> JsonRpcResponse {

#[allow(clippy::too_many_arguments)]
pub async fn handle_install(
    req: JsonRpcRequest,
    mcp: Option<McpManagerHandle>,
    cache: Arc<CatalogCache>,
    vault: Arc<SharedTokenManager>,
    marketplace: Arc<MarketplaceManager>,
) -> JsonRpcResponse {
```

- [ ] **Step 3: Delete `sources.rs` and its re-export**

```bash
git rm src/gateway/handlers/extensions/sources.rs
```
In `src/gateway/handlers/extensions/mod.rs` remove the line `pub mod sources;`.

- [ ] **Step 4: bin `builder/handlers/extensions.rs` — drop registry + sources registration**

Remove `use alephcore::hub::provider::ProviderRegistry;`. Drop `registry: Arc<ProviderRegistry>` from `register_extensions_install_handlers` and from its `extensions.disclosure` / `extensions.configure` / `extensions.install` closures (stop cloning/passing `registry`; call `handle_disclosure(req, cache)`, `handle_configure(req, cache)`, `handle_install(req, mcp, cache, vault, marketplace)`). **Delete the entire `register_extensions_sources_handlers` function.**

- [ ] **Step 5: bin `start/mod.rs` — AlephHubCatalog instead of the registry**

Replace the registry build + sources registration:
```rust
                let aleph_hub = std::sync::Arc::new(
                    alephcore::hub::catalog_client::AlephHubCatalog::new(
                        alephcore::hub::catalog_client::ALEPH_HUB_ID,
                        alephcore::hub::catalog_client::ALEPH_HUB_NAME,
                        alephcore::hub::catalog_client::ALEPH_HUB_URL,
                        alephcore::hub::types::TrustTier::Verified,
                    ),
                );
```
(the `register_extensions_sources_handlers(...)` call is removed entirely.)

Drop the `registry.clone()` argument from the `register_extensions_install_handlers(...)` call.

Replace the 6h sync loop body:
```rust
                {
                    let aleph_hub = aleph_hub.clone();
                    let cache = cache.clone();
                    tokio::spawn(async move {
                        let mut tick =
                            tokio::time::interval(std::time::Duration::from_secs(6 * 60 * 60));
                        loop {
                            tick.tick().await;
                            let report = aleph_hub.sync_into(&cache).await;
                            tracing::info!(
                                synced = report.synced,
                                failed = ?report.failed,
                                "extensions catalog sync"
                            );
                        }
                    });
                }
```
> Keep the independently-built `MarketplaceManager` + `marketplace_configs` (install backend) exactly as they are.

- [ ] **Step 6: Verify lib + bin compile together**

Run: `cargo check -p alephcore --bin aleph-server`
Expected: no errors. (This compiles the lib and the bin in one pass.)

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "hub: single-source gateway + startup wiring; remove extensions.sources.*"
```

---

### Task 5: Delete the dead provider tree + dedup/display/categorize

**Files:**
- Delete: `src/hub/provider/` (entire directory: `mod.rs`, `registry_builder.rs`, `static_hub.rs`, `mcp_registry.rs`, `docker_mcp.rs`, `marketplace.rs`)
- Delete: `src/hub/dedup.rs`, `src/hub/display.rs`, `src/hub/categorize.rs`
- Modify: `src/hub/mod.rs`

- [ ] **Step 1: Confirm zero consumers remain**

Run: `rg -n "SourceProvider|ProviderRegistry|build_default_registry|hub::dedup|hub::display|hub::categorize|dedup_by_priority|source_label\(" src`
Expected: matches ONLY inside the files about to be deleted (and `display::source_label` should have none left in `catalog.rs`). If anything else shows up, fix that consumer before deleting.

- [ ] **Step 2: Delete the files**

```bash
git rm -r src/hub/provider
git rm src/hub/dedup.rs src/hub/display.rs src/hub/categorize.rs
```

- [ ] **Step 3: Rewrite `src/hub/mod.rs`**

```rust
//! Unified Extensions Hub: one user-facing `Extension` concept over the
//! existing plugin / MCP / skill backends, fed by the single published Aleph
//! Hub catalog. See
//! docs/superpowers/specs/2026-06-20-aleph-hub-single-source-design.md
pub mod cache;
pub mod catalog_client;
pub mod hub_catalog;
pub mod install;
pub mod reconcile;
pub mod secrets;
pub mod trust;
pub mod types;
pub mod verify;

pub use types::{
    EnvDecl, ExtensionCategory, ExtensionEntry, ExtensionKind, HeaderDecl, InstallSpec,
    McpTransport, TrustTier,
};
```
> Keep the existing `pub use types::{…}` list verbatim — only the `pub mod` lines change. Verify the re-export list matches the current file before overwriting.

- [ ] **Step 4: Mark the superseded federation spec**

Prepend to `docs/superpowers/specs/2026-06-20-extension-hub-federation-design.md` (force-add when committing, the dir is gitignored):
```markdown
> **SUPERSEDED (2026-06-20)** by `2026-06-20-aleph-hub-single-source-design.md`.
> The local federation / multi-source / dedup design below was reversed: Aleph
> is now a single-source consumer of one published Aleph Hub catalog.
```

- [ ] **Step 5: Verify lib + bin compile**

Run: `cargo check -p alephcore --bin aleph-server`
Expected: no errors.

- [ ] **Step 6: Commit**

```bash
git add -A
git add -f docs/superpowers/specs/2026-06-20-extension-hub-federation-design.md
git commit -m "hub: delete provider registry, dedup, display, categorize (single source)"
```

---

### Task 6: Panel rename to "Aleph Hub" + remove dead sources stubs

**Files:**
- Modify: `interfaces/webchat/locales/en.json`, `interfaces/webchat/locales/zh.json`
- Modify: `interfaces/webchat/src/api/extensions.rs`

- [ ] **Step 1: Locate the exact keys**

Run: `rg -n '"extensions"|"title"|"subtitle"' interfaces/webchat/locales/en.json | head` and the `nav` section. You need `nav.extensions`, `extensions.title`, `extensions.subtitle`.

- [ ] **Step 2: Rename in `en.json`** — `nav.extensions` and `extensions.title` → `"Aleph Hub"`; `extensions.subtitle` (`"Curated by your Store Agent"`) → `"Discover and install extensions"`.

- [ ] **Step 3: Rename in `zh.json`** — `nav.extensions` and `extensions.title` → `"Aleph Hub"`; `extensions.subtitle` (`"由你的商店智能体策展"`) → `"发现并安装扩展"`.

> No code change in `nav_menu.rs` or `views/extensions/mod.rs` — both pull the label/title from i18n, so they update automatically.

- [ ] **Step 4: Confirm the sources stubs have no callers, then remove them**

Run: `rg -n "sources_list|sources_refresh|SourceInfo" interfaces/webchat/src`
Expected: only definitions in `interfaces/webchat/src/api/extensions.rs` (no calls from `views/`). If a view calls them, stop and reassess. Otherwise delete the `SourceInfo` struct and the `sources_list` / `sources_refresh` methods from `api/extensions.rs`. Leave the panel `ExtensionEntry` mirror (with `source_label: String`) untouched.

- [ ] **Step 5: Verify the panel compiles (WASM target)**

Run: `cargo check -p aleph-panel --target wasm32-unknown-unknown`
Expected: no errors. (Do NOT run `just wasm`.)

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "hub: rename Extensions panel to Aleph Hub; drop dead sources stubs"
```

---

## Post-implementation (orchestrator, not a code task)

- Update the long-term memory `extensions-store-progress` (federation → single-source consumer; `AlephHubCatalog`; `via`/`install_spec` on the cached entry; sources RPC removed; escape hatch deferred).
- The runtime won't reflect panel/handler changes until the server binary is rebuilt and the panel re-embedded (`just wasm` + rebuild) — out of scope here, do only when the user asks.
- Fill the real `hub.heyaleph.com` artifact once the Aleph-Hub site is live (the const is already set; the site is a separate project).

---

## Self-Review

**Spec coverage:** D1 single source (T4/T5 remove browse providers + registry). D2 collapse (T2 client + T5 delete). D3 provenance via `via` (T1 field + into_entry, T4 catalog handler). D4 categories prefilled / delete categorize (T5). D5 delete dedup (T4 stops using it, T5 deletes). D6 install_spec in cache (T1 + T3 + T4 cache-lookup resolve). D7 escape hatch deferred (out of scope, noted). D8 rename (T6). D9 remove sources RPC (T4). D10 single URL const (T2). D11 forward-compat (`via` serde-default; kinds untouched). All covered.

**Placeholder scan:** No TBD/TODO; every code step shows complete before/after. Two spots ask the implementer to verify real field/struct names against live code before finalizing (catalog fixture JSON; the `pub use` list / tool struct fields) — these are verification gates, not placeholders.

**Type consistency:** `SyncReport { synced: usize, failed: Vec<String> }` is produced in T2 and consumed identically in T3 (`HubCatalogSyncOutput`) and T4 (startup tracing) — `sync_into` returns it directly (no `Result`). `ExtensionEntry.install_spec: Option<InstallSpec>` defined T1, populated by `into_entry` T1, read in T3 tools + T4 handler. `via` defined T1, populated T1, emitted as `source_label` wire key in T4. `AlephHubCatalog::{new,fetch,sync_into}` + `ALEPH_HUB_ID/NAME/URL` defined T2, used T3/T4. Names align across tasks.
