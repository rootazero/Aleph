# Extension Hub Federation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rename the `store` extension subsystem to `hub`, retire the protected store agent (demoting its tools to the main loop behind trust + operator-tier gating), add a `StaticHubProvider` that consumes a versioned static catalog artifact from Aleph Hub, make provenance (source badge + upstream repo link) first-class, deduplicate the same upstream extension across hubs by priority, and run catalog sync as a background task.

**Architecture:** The shipped "thin federation" already pulls browse data from fixed central sources into a local SQLite cache via `SourceProvider` impls. We add a `StaticHubProvider` (consumes a versioned JSON artifact = the Aleph Hub contract), retire the dedicated store agent so install/sync/resolve/verify become plain main-loop tools gated by existing trust rails + device-tier, surface provenance (which hub + upstream repo), and collapse cross-source duplicates at read time keyed on upstream repo with a configurable source priority.

**Tech Stack:** Rust (tokio, serde, rusqlite, reqwest, async-trait, futures), Leptos/WASM panel (interfaces/webchat), config.toml (serde).

## Global Constraints

- **MSRV = 1.95**; tokio is the only async runtime; serde is the only serialization stack; memory/catalog layer is sqlite only — do not add new heavy deps.
- **极度节制 cargo**: do NOT run full test suites. Verify with a single `cargo check -p alephcore --lib` at each phase boundary (not per-step). `alephcore` build is memory-heavy — never run parallel test builds.
- **Surgical rename**: touch ONLY the extension subsystem — `src/store/` and `src/builtin_tools/store/`. NEVER touch the unrelated same-named modules: `src/memory/store/`, `src/gateway/security/` `store` submodule, `src/projects/`, `src/providers/auth_profiles/`, `src/teams/snapshots/`, or any generic `store`/`storage`/`restore` identifier.
- **Secrets** reuse the existing `{{secret:NAME}}` vault pipeline (`SharedTokenManager`); never invent a parallel secret-ref scheme.
- **Provenance mandate (P-Provenance)**: every catalog entry must surface its source label (which hub) and `repo_url` (upstream author repo). Never obscure open-source provenance.
- **Browse-surface naming**: keep `extensions.*` RPC and the "Extensions" UI word unchanged. "Hub" is only the module + the source/federation concept.
- Single-branch development on `main`. Commit messages: `<scope>: <description>` (English).

---

## Phase 0 — Mechanical rename `store` → `hub`

Pure rename, zero behavior change, compile-verified, one commit. Grounding inventory: ~33 files, `crate::store::` ×73, `use crate::store` ×57, tool-name strings ×38, `STORE_TOOLS` ×7, `store_catalog.db` ×5.

### Task 0.1: Move directories + module declarations

**Files:**
- Rename dir: `src/store/` → `src/hub/`
- Rename dir: `src/builtin_tools/store/` → `src/builtin_tools/hub/`
- Modify: `src/lib.rs` (the extension-subsystem `pub mod store;` only)
- Modify: `src/builtin_tools/mod.rs` (the `pub mod store;` there)

- [ ] **Step 1: Git-move the two directories**

```bash
cd /d/Workspace/Aleph
git mv src/store src/hub
git mv src/builtin_tools/store src/builtin_tools/hub
```

- [ ] **Step 2: Update the two module declarations (extension subsystem ONLY)**

Find the exact lines (do NOT touch `mod store` in memory/security/projects/teams/auth_profiles):

```bash
grep -rn "^pub mod store;" src/lib.rs src/builtin_tools/mod.rs
```

In `src/lib.rs` change the extension-subsystem `pub mod store;` → `pub mod hub;`.
In `src/builtin_tools/mod.rs` change `pub mod store;` → `pub mod hub;`.

### Task 0.2: Rewrite module-path references `crate::store::` → `crate::hub::`

**Files (21):** `src/hub/cache.rs`, `categorize.rs`, `install.rs`, `provider/docker_mcp.rs`, `provider/marketplace.rs`, `provider/mcp_registry.rs`, `provider/mod.rs`, `provider/registry_builder.rs`, `reconcile.rs`, `secrets.rs`, `trust.rs`, `verify.rs`; `src/builtin_tools/hub/catalog_sync.rs`, `fetch_docs.rs`, `install_run.rs`, `install_verify.rs`, `resolve_spec.rs`; `src/executor/builtin_registry/config.rs`; `src/gateway/handlers/extensions/catalog.rs`, `install.rs`, `sources.rs`.

- [ ] **Step 1: Replace references**

For each file above, replace `crate::store::` → `crate::hub::` and `use crate::store` → `use crate::hub`. Targeted (not repo-wide) to avoid touching unrelated `store` modules:

```bash
for f in src/hub/*.rs src/hub/provider/*.rs src/builtin_tools/hub/*.rs \
         src/executor/builtin_registry/config.rs \
         src/gateway/handlers/extensions/catalog.rs \
         src/gateway/handlers/extensions/install.rs \
         src/gateway/handlers/extensions/sources.rs; do
  sed -i 's/crate::store::/crate::hub::/g; s/use crate::store/use crate::hub/g' "$f"
done
```

- [ ] **Step 2: Rename the 5 tool struct types**

Rename struct identifiers across the renamed tool module and its consumers (constructor + registry metadata):
`StoreCatalogSyncTool→HubCatalogSyncTool`, `StoreFetchDocsTool→HubFetchDocsTool`, `StoreResolveSpecTool→HubResolveSpecTool`, `StoreInstallRunTool→HubInstallRunTool`, `StoreInstallVerifyTool→HubInstallVerifyTool`.

```bash
for f in src/builtin_tools/hub/*.rs \
         src/executor/builtin_registry/builder/constructor/mod.rs \
         src/executor/builtin_registry/definitions.rs; do
  sed -i 's/StoreCatalogSyncTool/HubCatalogSyncTool/g; s/StoreFetchDocsTool/HubFetchDocsTool/g; s/StoreResolveSpecTool/HubResolveSpecTool/g; s/StoreInstallRunTool/HubInstallRunTool/g; s/StoreInstallVerifyTool/HubInstallVerifyTool/g' "$f"
done
```

Also fix any `crate::builtin_tools::store::` paths in the constructor:
```bash
sed -i 's/crate::builtin_tools::store::/crate::builtin_tools::hub::/g' src/executor/builtin_registry/builder/constructor/mod.rs
```

### Task 0.3: Rename tool name strings `store_*` → `hub_*` and `STORE_TOOLS` → `HUB_TOOLS`

**Files:** `src/builtin_tools/hub/{catalog_sync,fetch_docs,resolve_spec,install_run,install_verify}.rs` (the `const NAME`), `src/executor/builtin_registry/definitions.rs` (5 names), `src/executor/builtin_registry/registry/tool_registry_impl.rs` (5 dispatch guards), `src/agents/registry.rs` (main `denied_tools` ×5, store-agent block, tests), `src/agents/tool_sets.rs` (`STORE_TOOLS` list contents + const name + `resolve` match + test).

- [ ] **Step 1: Replace the 5 tool-name string literals**

```bash
for f in src/builtin_tools/hub/catalog_sync.rs src/builtin_tools/hub/fetch_docs.rs \
         src/builtin_tools/hub/resolve_spec.rs src/builtin_tools/hub/install_run.rs \
         src/builtin_tools/hub/install_verify.rs \
         src/executor/builtin_registry/definitions.rs \
         src/executor/builtin_registry/registry/tool_registry_impl.rs \
         src/agents/registry.rs src/agents/tool_sets.rs; do
  sed -i 's/store_catalog_sync/hub_catalog_sync/g; s/store_fetch_docs/hub_fetch_docs/g; s/store_resolve_spec/hub_resolve_spec/g; s/store_install_run/hub_install_run/g; s/store_install_verify/hub_install_verify/g' "$f"
done
```

- [ ] **Step 2: Rename the `STORE_TOOLS` constant → `HUB_TOOLS`**

```bash
sed -i 's/STORE_TOOLS/HUB_TOOLS/g' src/agents/tool_sets.rs src/agents/registry.rs
```

This updates: the const definition + doc comment (tool_sets.rs:42-50), the `"STORE_TOOLS" => Some(...)` resolve arm (tool_sets.rs:60), the test (tool_sets.rs:146), and the store agent's `with_allowed_tool_sets(vec!["STORE_TOOLS".into()])` (registry.rs:323). Update the resolve match string `"STORE_TOOLS"` → `"HUB_TOOLS"` and the agent's `with_allowed_tool_sets(vec!["HUB_TOOLS".into()])` accordingly (sed covers both).

### Task 0.4: Rename on-disk cache file + verify + commit

**Files:** `src/bin/aleph-server/commands/start/mod.rs` (lines ~785-813 and ~1321-1324).

- [ ] **Step 1: Rename the cache filename**

```bash
sed -i 's/store_catalog\.db/hub_catalog.db/g' src/bin/aleph-server/commands/start/mod.rs
```

(The cache is rebuildable on next sync; no data migration needed. Old `store_catalog.db` is simply abandoned.)

- [ ] **Step 2: Compile-verify the whole rename**

Run: `cargo check -p alephcore --lib`
Expected: clean compile (warnings ok). Fix any missed `crate::store::` / struct-name / string references the grep missed.

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "refactor(hub): rename store extension subsystem to hub"
```

---

## Phase 1 — Retire the protected store agent + demote tools

After this phase no `store` agent exists; the 5 `hub_*` tools are available to the `main` loop, with `hub_install_run` gated to operator tier.

### Task 1.1: Remove the built-in store agent

**Files:**
- Modify: `src/agents/registry.rs` — remove the store `AgentDef` (was lines 311-324), the alias arm (was line 204), update `test_builtin_agents_count` (8→7), update the subagent-enumerating tests that reference `"store"`.
- Modify: `src/builtin_tools/agent_manage/delete.rs` — remove `"store"` from the `is_protected_rejects_all_builtins` test list (keep `"main"` + others).

**Interfaces:**
- Produces: `builtin_agents()` returns 7 agents; no agent id `"store"` resolvable.

- [ ] **Step 1: Remove the store agent definition**

In `src/agents/registry.rs`, delete the entire `AgentDef::new("store", AgentMode::SubAgent)…with_max_iterations(15)` block (the store entry in `builtin_agents()`). Remove the `"store" => return Some("store"),` alias arm.

- [ ] **Step 2: Update the count + protection tests**

In `src/agents/registry.rs::test_builtin_agents_count`, change `assert_eq!(agents.len(), 8);` → `assert_eq!(agents.len(), 7);` and delete the block that finds + asserts the `"store"` agent. In any test enumerating subagents for descriptions/when_to_use, drop the `"store"` expectation.

In `src/builtin_tools/agent_manage/delete.rs::is_protected_rejects_all_builtins`, remove `"store",` from the id array (keep `main`, `explore`, `coder`, `researcher`, `default`, `plan`, `verify`).

### Task 1.2: Demote the 5 hub tools to the main loop

**Files:** `src/agents/registry.rs` — `main` (and `verify`) `denied_tools`; `src/agents/registry.rs::wildcard_agents_deny_store_tools` test.

**Interfaces:**
- Consumes: `main` has `allowed_tools = ["*", "flow_run"]` (wildcard).
- Produces: `main.is_tool_allowed("hub_install_run") == true` (wildcard now reaches it); install safety enforced by the operator gate (Task 1.3), not by agent scoping.

- [ ] **Step 1: Remove hub tools from `denied_tools`**

In `src/agents/registry.rs`, in the `main` `AgentDef`, delete the 5 `denied_tools` entries (`hub_catalog_sync`, `hub_fetch_docs`, `hub_resolve_spec`, `hub_install_run`, `hub_install_verify`). Do the same for the `verify` agent if it lists them. Leave other denied entries untouched.

- [ ] **Step 2: Update the (now-inverted) test**

Rename/rewrite `wildcard_agents_deny_store_tools` → `wildcard_agents_allow_hub_tools`: assert `main.is_tool_allowed("hub_catalog_sync")` etc. are now `true`, and keep the `assert!(main.is_tool_allowed("flow_run"))` line.

- [ ] **Step 3: Decide HUB_TOOLS const fate**

`HUB_TOOLS` is no longer referenced by any agent's `allowed_tool_sets` (the store agent owned it). Keep the const + its `resolve()` arm for the grouping/back-compat, but remove its now-dead test if it asserts store-agent scoping. (No behavior depends on it.)

### Task 1.3: Gate `hub_install_run` to operator tier

**Files:** `src/gateway/method_authz.rs` — `OPERATOR_TOOLS` list + `config_tools_require_operator` test.

**Interfaces:**
- Consumes: the dispatch gate at `src/tools/scoped/dispatch.rs:138-186` already enforces `tool_requires_operator(name)`.
- Produces: `tool_requires_operator("hub_install_run") == true`; chat-tier callers are suspended for operator approval (or fail closed).

- [ ] **Step 1: Add the install tool to the operator list**

In `src/gateway/method_authz.rs`, add `"hub_install_run",` to the `OPERATOR_TOOLS` array (alongside `vault_store`, `skill_install`, `clawhub`, etc.).

- [ ] **Step 2: Add it to the operator test**

In `config_tools_require_operator`, add `"hub_install_run"` to the asserted list.

- [ ] **Step 3: Verify the panel RPC path is also tiered (CHECK)**

Read `src/bin/aleph-server/commands/start/builder/handlers/extensions.rs` + `src/gateway/handlers/extensions/install.rs`. Confirm `extensions.install` (the panel user-gesture path) is rejected for chat-tier devices. If it is NOT gated, add a `caller_is_operator()` check at the handler entry (mirroring `current_turn_context().is_none_or(|t| t.caller_is_operator())`). If it already routes through tool dispatch, no change needed — note the finding in the commit message.

- [ ] **Step 4: Compile-verify + commit**

Run: `cargo check -p alephcore --lib`
Expected: clean.

```bash
git add -A
git commit -m "hub: retire store agent, demote hub tools to main loop behind operator gate"
```

---

## Phase 2 — Hub catalog contract + StaticHubProvider + provenance

### Task 2.1: Hub catalog artifact types + normalize

**Files:**
- Create: `src/hub/hub_catalog.rs`
- Modify: `src/hub/mod.rs` (add `pub mod hub_catalog;`)
- Test: inline `#[cfg(test)]` in `hub_catalog.rs`

**Interfaces:**
- Produces: `HubCatalogManifest`, `HubCatalogEntry`, `HubCatalogArtifact`, and `fn HubCatalogEntry::into_entry(&self, hub_id: &str) -> ExtensionEntry`.

- [ ] **Step 1: Write the failing test**

```rust
// src/hub/hub_catalog.rs (bottom)
#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub::types::{ExtensionKind, TrustTier};

    const FIXTURE: &str = r#"{
      "manifest": {"schema_version":1,"hub_id":"aleph-hub","name":"Aleph Hub","generated_at":"2026-06-20T00:00:00Z","entry_count":1},
      "entries": [{
        "id":"aleph-hub:acme/foo","kind":"mcp","category":"developer","name":"Acme Foo",
        "description":"d","repo_url":"https://github.com/acme/foo","trust_tier":"verified",
        "requires_config":false,
        "install_spec":{"type":"mcp_stdio","command":"npx","args":["@acme/foo"]}
      }]
    }"#;

    #[test]
    fn parses_and_normalizes() {
        let art: HubCatalogArtifact = serde_json::from_str(FIXTURE).unwrap();
        assert_eq!(art.manifest.schema_version, 1);
        let e = art.entries[0].into_entry(&art.manifest.hub_id);
        assert_eq!(e.source_id, "aleph-hub");
        assert_eq!(e.kind, ExtensionKind::Mcp);
        assert_eq!(e.trust_tier, TrustTier::Verified);
        assert_eq!(e.repo_url.as_deref(), Some("https://github.com/acme/foo"));
        assert!(!e.installed && !e.enabled); // per-user fields default false
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p alephcore --lib hub_catalog::tests::parses_and_normalizes`
Expected: FAIL (types not defined).

- [ ] **Step 3: Implement the types + normalize**

```rust
// src/hub/hub_catalog.rs
//! Wire format for a versioned static Hub catalog artifact (the contract
//! produced by Aleph-Hub and consumed by `StaticHubProvider`). Objective
//! subset of `ExtensionEntry` — no per-user state.

use serde::Deserialize;
use crate::hub::types::{
    ExtensionCategory, ExtensionEntry, ExtensionKind, InstallSpec, TrustTier,
};

#[derive(Debug, Clone, Deserialize)]
pub struct HubCatalogArtifact {
    pub manifest: HubCatalogManifest,
    #[serde(default)]
    pub entries: Vec<HubCatalogEntry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HubCatalogManifest {
    pub schema_version: u32,
    pub hub_id: String,
    pub name: String,
    #[serde(default)]
    pub generated_at: Option<String>,
    #[serde(default)]
    pub entry_count: Option<u64>,
    #[serde(default)]
    pub content_hash: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HubCatalogEntry {
    pub id: String,
    pub kind: ExtensionKind,
    pub category: ExtensionCategory,
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub version: Option<String>,
    /// Upstream author repo (open-source attribution). Required by contract.
    pub repo_url: Option<String>,
    pub trust_tier: TrustTier,
    #[serde(default)]
    pub requires_config: bool,
    #[serde(default)]
    pub config_schema: Option<serde_json::Value>,
    pub install_spec: InstallSpec,
}

/// Current artifact schema version this client understands.
pub const SUPPORTED_SCHEMA_VERSION: u32 = 1;

impl HubCatalogEntry {
    /// Project the objective wire record into the cache's `ExtensionEntry`,
    /// stamping the source and zeroing per-user state.
    #[must_use]
    pub fn into_entry(&self, hub_id: &str) -> ExtensionEntry {
        ExtensionEntry {
            id: self.id.clone(),
            kind: self.kind,
            category: self.category,
            name: self.name.clone(),
            description: self.description.clone(),
            author: self.author.clone(),
            icon: self.icon.clone(),
            tags: self.tags.clone(),
            version: self.version.clone(),
            source_id: hub_id.to_string(),
            repo_url: self.repo_url.clone(),
            trust_tier: self.trust_tier,
            requires_config: self.requires_config,
            config_schema: self.config_schema.clone(),
            installed: false,
            enabled: false,
            update_available: false,
        }
    }
}
```

Add `pub mod hub_catalog;` to `src/hub/mod.rs`. Note `into_entry` takes `&self` (not consuming) so the provider can also keep the original `install_spec` for `resolve_install_spec`.

- [ ] **Step 4: Run the test (PASS)**

Run: `cargo test -p alephcore --lib hub_catalog::tests::parses_and_normalizes`
Expected: PASS.

### Task 2.2: `StaticHubProvider`

**Files:**
- Create: `src/hub/provider/static_hub.rs`
- Modify: `src/hub/provider/mod.rs` (`pub mod static_hub;` + add `display_name` default to the trait — see Task 2.3)
- Test: inline tests using a small in-process fixture string (no network).

**Interfaces:**
- Consumes: `SourceProvider` trait, `HubCatalogArtifact`, `scan_for_injection` (`src/hub/trust.rs`).
- Produces: `StaticHubProvider { id, name, artifact_url, trust_tier, http }` with `new(...)`; holds the last-synced `install_spec` per id for `resolve_install_spec`.

- [ ] **Step 1: Implement the provider**

```rust
// src/hub/provider/static_hub.rs
use std::collections::HashMap;
use std::sync::Mutex;

use crate::hub::hub_catalog::{HubCatalogArtifact, SUPPORTED_SCHEMA_VERSION};
use crate::hub::provider::{Query, SourceError, SourceProvider, SyncCtx};
use crate::hub::trust::scan_for_injection;
use crate::hub::types::{ExtensionEntry, ExtensionKind, InstallSpec, TrustTier};

/// Consumes a versioned static Hub catalog artifact (the Aleph-Hub contract).
pub struct StaticHubProvider {
    id: String,
    name: String,
    artifact_url: String,
    trust_tier: TrustTier,
    http: reqwest::Client,
    /// install_spec by entry id, captured at sync for resolve_install_spec.
    specs: Mutex<HashMap<String, InstallSpec>>,
}

impl StaticHubProvider {
    #[must_use]
    pub fn new(id: String, name: String, artifact_url: String, trust_tier: TrustTier) -> Self {
        Self {
            id,
            name,
            artifact_url,
            trust_tier,
            http: reqwest::Client::new(),
            specs: Mutex::new(HashMap::new()),
        }
    }

    /// Parse + normalize an artifact body. Split out for tests (no network).
    fn ingest(&self, body: &str) -> Result<Vec<ExtensionEntry>, SourceError> {
        let art: HubCatalogArtifact =
            serde_json::from_str(body).map_err(|e| SourceError::Parse(e.to_string()))?;
        if art.manifest.schema_version > SUPPORTED_SCHEMA_VERSION {
            return Err(SourceError::Other(format!(
                "hub '{}' schema_version {} > supported {}",
                self.id, art.manifest.schema_version, SUPPORTED_SCHEMA_VERSION
            )));
        }
        let mut specs = self.specs.lock().unwrap_or_else(|e| e.into_inner());
        specs.clear();
        let mut out = Vec::with_capacity(art.entries.len());
        for he in &art.entries {
            // Defense in depth: scan curated text for hidden-instruction attacks.
            let findings = scan_for_injection(&format!("{} {}", he.name, he.description));
            if !findings.is_empty() {
                tracing::warn!(hub = %self.id, id = %he.id, ?findings, "hub entry injection findings");
            }
            specs.insert(he.id.clone(), he.install_spec.clone());
            out.push(he.into_entry(&art.manifest.hub_id));
        }
        Ok(out)
    }
}

#[async_trait::async_trait]
impl SourceProvider for StaticHubProvider {
    fn id(&self) -> &str {
        &self.id
    }
    fn display_name(&self) -> &str {
        &self.name
    }
    fn kinds(&self) -> &[ExtensionKind] {
        &[ExtensionKind::Skill, ExtensionKind::Plugin, ExtensionKind::Mcp]
    }
    fn trust_tier(&self) -> TrustTier {
        self.trust_tier
    }

    async fn sync(&self, _ctx: &SyncCtx) -> Result<Vec<ExtensionEntry>, SourceError> {
        let resp = self
            .http
            .get(&self.artifact_url)
            .timeout(std::time::Duration::from_secs(30))
            .send()
            .await
            .map_err(|e| SourceError::Network(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(SourceError::Network(format!("HTTP {}", resp.status())));
        }
        let body = resp.text().await.map_err(|e| SourceError::Network(e.to_string()))?;
        self.ingest(&body)
    }

    async fn search(&self, _q: &Query) -> Option<Result<Vec<ExtensionEntry>, SourceError>> {
        None // browse is served from the local cache
    }

    async fn resolve_install_spec(
        &self,
        entry: &ExtensionEntry,
    ) -> Result<InstallSpec, SourceError> {
        let specs = self.specs.lock().unwrap_or_else(|e| e.into_inner());
        specs
            .get(&entry.id)
            .cloned()
            .ok_or_else(|| SourceError::Other(format!("no install spec cached for '{}'", entry.id)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ingest_normalizes_and_caches_spec() {
        let p = StaticHubProvider::new(
            "aleph-hub".into(),
            "Aleph Hub".into(),
            "http://unused".into(),
            TrustTier::Verified,
        );
        let body = r#"{"manifest":{"schema_version":1,"hub_id":"aleph-hub","name":"Aleph Hub"},
          "entries":[{"id":"aleph-hub:acme/foo","kind":"mcp","category":"developer","name":"Foo",
          "description":"d","repo_url":"https://github.com/acme/foo","trust_tier":"verified",
          "install_spec":{"type":"mcp_stdio","command":"npx","args":["@acme/foo"]}}]}"#;
        let entries = p.ingest(body).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].source_id, "aleph-hub");
        // spec captured for resolve
        assert!(p.specs.lock().unwrap().contains_key("aleph-hub:acme/foo"));
    }

    #[test]
    fn ingest_rejects_future_schema() {
        let p = StaticHubProvider::new("h".into(), "H".into(), "x".into(), TrustTier::Community);
        let body = r#"{"manifest":{"schema_version":999,"hub_id":"h","name":"H"},"entries":[]}"#;
        assert!(matches!(p.ingest(body), Err(SourceError::Other(_))));
    }
}
```

- [ ] **Step 2: Wire module + run tests**

Add `pub mod static_hub;` to `src/hub/provider/mod.rs`.
Run: `cargo test -p alephcore --lib provider::static_hub::tests`
Expected: PASS (both tests).

### Task 2.3: Add `display_name` to the `SourceProvider` trait

**Files:** `src/hub/provider/mod.rs`; `src/hub/provider/{mcp_registry,docker_mcp,marketplace}.rs`.

**Interfaces:**
- Produces: `SourceProvider::display_name(&self) -> &str` (default returns `id()`); `ProviderRegistry::list_sources()` extended to include the display name.

- [ ] **Step 1: Add the trait method (default)**

In the `SourceProvider` trait, add:
```rust
/// Human-facing source label for provenance badges. Defaults to `id()`.
fn display_name(&self) -> &str {
    self.id()
}
```

- [ ] **Step 2: Override per built-in provider**

`McpRegistryProvider::display_name` → `"MCP Registry"`; `DockerMcpProvider` → `"Docker"`; `MarketplaceProvider` → `"Plugin Marketplace"`. (`StaticHubProvider` already overrides with its manifest name.)

- [ ] **Step 3: Extend `list_sources` to carry the label**

Change `ProviderRegistry::list_sources` to return `Vec<(String, String, TrustTier, Vec<ExtensionKind>)>` (id, display_name, tier, kinds). Update the one caller `src/gateway/handlers/extensions/sources.rs::handle_list` to emit `"name": display_name` in its JSON.

### Task 2.4: Register Aleph Hub built-in + config-driven extra hubs

**Files:**
- Modify: `src/hub/provider/registry_builder.rs`
- Modify: `src/config/structs.rs` (add `[extension_hubs]` config table)
- Modify: `src/bin/aleph-server/commands/start/mod.rs` (pass hub configs into `build_default_registry`)

**Interfaces:**
- Consumes: `HashMap<String, MarketplaceConfig>` (existing), plus new `Vec<ExtensionHubEntry>`.
- Produces: registry includes `StaticHubProvider` for Aleph Hub + any config hubs.

- [ ] **Step 1: Add the config struct**

In `src/config/structs.rs`, mirroring `PluginMarketplaceEntry`:
```rust
/// Extra extension hub (static catalog artifact) for config.toml.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct ExtensionHubEntry {
    /// Stable source id, e.g. "hermes-atlas".
    pub id: String,
    /// Human label for provenance badge, e.g. "Hermes Atlas".
    pub name: String,
    /// URL of the versioned static catalog artifact (JSON).
    pub url: String,
    /// Trust tier: "official" | "verified" | "community" | "unverified".
    #[serde(default = "default_hub_tier")]
    pub trust_tier: String,
}
fn default_hub_tier() -> String { "community".to_string() }
```
Add to the config root: `#[serde(default)] pub extension_hubs: Vec<ExtensionHubEntry>,`.

- [ ] **Step 2: Register Aleph Hub + config hubs in the builder**

Change `build_default_registry` signature to also accept `hubs: Vec<ExtensionHubEntry>` and register:
```rust
// Built-in Aleph Hub (always present).
reg.register(Box::new(StaticHubProvider::new(
    "aleph-hub".into(),
    "Aleph Hub".into(),
    aleph_hub_url(), // const default artifact URL (Aleph-Hub publish endpoint)
    TrustTier::Verified,
)));
for h in hubs {
    let tier = TrustTier::from_str_or(&h.trust_tier, TrustTier::Community);
    reg.register(Box::new(StaticHubProvider::new(h.id, h.name, h.url, tier)));
}
```
Add a `const ALEPH_HUB_URL: &str = "https://hub.aleph.<domain>/catalog.json";` placeholder (final URL TBD — leave a clearly-marked constant) and a `TrustTier::from_str_or` helper in `types.rs` if not present.

- [ ] **Step 3: Thread config at startup**

In `src/bin/aleph-server/commands/start/mod.rs`, read `cfg.extension_hubs.clone()` next to the existing `plugin_marketplaces` read, and pass it to `build_default_registry(marketplace_configs, hubs)`. Update the `HubCatalogSyncTool` construction sites to pass hubs too (so background/manual sync includes them) — see Task 5.

- [ ] **Step 4: Compile-verify + commit**

Run: `cargo check -p alephcore --lib`

```bash
git add -A
git commit -m "hub: StaticHubProvider + Aleph Hub builtin + config-driven hubs + provenance display_name"
```

---

## Phase 3 — Cross-source dedup core

### Task 3.1: `hub::dedup` pure function

**Files:**
- Create: `src/hub/dedup.rs`
- Modify: `src/hub/mod.rs` (`pub mod dedup;`)
- Test: inline.

**Interfaces:**
- Produces: `pub const DEFAULT_HUB_PRIORITY: &[&str]`; `pub fn dedup_by_priority(entries: Vec<ExtensionEntry>, order: &[String]) -> Vec<ExtensionEntry>`.

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub::types::{ExtensionCategory, ExtensionEntry, ExtensionKind, TrustTier};

    fn e(id: &str, src: &str, repo: Option<&str>) -> ExtensionEntry {
        ExtensionEntry {
            id: id.into(), kind: ExtensionKind::Mcp, category: ExtensionCategory::Other,
            name: id.into(), description: String::new(), author: None, icon: None,
            tags: vec![], version: None, source_id: src.into(),
            repo_url: repo.map(Into::into), trust_tier: TrustTier::Community,
            requires_config: false, config_schema: None,
            installed: false, enabled: false, update_available: false,
        }
    }

    #[test]
    fn keeps_highest_priority_duplicate() {
        let order: Vec<String> = ["aleph-hub","clawhub","hermes-atlas"].iter().map(|s| s.to_string()).collect();
        let input = vec![
            e("clawhub:foo", "clawhub", Some("https://github.com/acme/foo")),
            e("aleph-hub:foo", "aleph-hub", Some("https://github.com/acme/foo.git")),
            e("hermes-atlas:foo", "hermes-atlas", Some("https://github.com/acme/foo/")),
            e("docker-mcp:bar", "docker-mcp", None), // no repo → never deduped
        ];
        let out = dedup_by_priority(input, &order);
        assert_eq!(out.len(), 2);
        let foo = out.iter().find(|x| x.repo_url.is_some()).unwrap();
        assert_eq!(foo.source_id, "aleph-hub"); // highest priority wins despite .git/slash variance
        assert!(out.iter().any(|x| x.source_id == "docker-mcp"));
    }
}
```

- [ ] **Step 2: Run to verify FAIL**

Run: `cargo test -p alephcore --lib dedup::tests::keeps_highest_priority_duplicate`
Expected: FAIL (not defined).

- [ ] **Step 3: Implement**

```rust
// src/hub/dedup.rs
//! Cross-source dedup: the same upstream extension surfaced by multiple hubs
//! collapses to the highest-priority source. Keyed on normalized repo_url.

use std::collections::HashMap;
use crate::hub::types::ExtensionEntry;

/// Default source priority (earlier = higher). Overridable via config.
pub const DEFAULT_HUB_PRIORITY: &[&str] = &["aleph-hub", "clawhub", "hermes-atlas"];

/// Normalized upstream identity, or None when there is no resolvable upstream
/// (such entries are never cross-deduped).
#[must_use]
pub fn dedup_key(entry: &ExtensionEntry) -> Option<String> {
    let raw = entry.repo_url.as_deref()?.trim().to_ascii_lowercase();
    let s = raw
        .strip_prefix("https://")
        .or_else(|| raw.strip_prefix("http://"))
        .unwrap_or(&raw);
    let s = s.strip_suffix('/').unwrap_or(s);
    let s = s.strip_suffix(".git").unwrap_or(s);
    let s = s.strip_suffix('/').unwrap_or(s);
    Some(s.to_string())
}

#[must_use]
fn rank(source_id: &str, order: &[String]) -> usize {
    order.iter().position(|s| s == source_id).unwrap_or(usize::MAX)
}

/// Collapse cross-source duplicates, keeping the best-ranked source per
/// upstream. Entries without a dedup key pass through untouched. Stable: first
/// occurrence position is preserved for winners; non-keyed entries are appended.
#[must_use]
pub fn dedup_by_priority(entries: Vec<ExtensionEntry>, order: &[String]) -> Vec<ExtensionEntry> {
    let mut idx_of: HashMap<String, usize> = HashMap::new();
    let mut winners: Vec<ExtensionEntry> = Vec::new();
    let mut passthrough: Vec<ExtensionEntry> = Vec::new();
    for e in entries {
        match dedup_key(&e) {
            None => passthrough.push(e),
            Some(k) => match idx_of.get(&k).copied() {
                Some(i) => {
                    let cur = rank(&winners[i].source_id, order);
                    let new = rank(&e.source_id, order);
                    if new < cur || (new == cur && e.source_id < winners[i].source_id) {
                        winners[i] = e;
                    }
                }
                None => {
                    idx_of.insert(k, winners.len());
                    winners.push(e);
                }
            },
        }
    }
    winners.append(&mut passthrough);
    winners
}
```
Add `pub mod dedup;` to `src/hub/mod.rs`.

- [ ] **Step 4: Run the test (PASS) + commit**

Run: `cargo test -p alephcore --lib dedup::tests`
Expected: PASS.

```bash
git add -A
git commit -m "hub: cross-source dedup by upstream repo with source priority"
```

---

## Phase 4 — Gateway provenance + dedup wiring

### Task 4.1: Apply dedup + source label in `extensions.catalog`

**Files:**
- Modify: `src/gateway/handlers/extensions/catalog.rs`
- Modify: `src/bin/aleph-server/commands/start/builder/handlers/extensions.rs` (pass priority order into the handler if needed)
- Create: `src/hub/display.rs` (static source label fallback) + `pub mod display;` in `src/hub/mod.rs`

**Interfaces:**
- Consumes: `hub::dedup::dedup_by_priority`, `DEFAULT_HUB_PRIORITY`.
- Produces: `extensions.catalog` entries carry `source_label`; duplicates collapsed.

- [ ] **Step 1: Source label fallback helper**

```rust
// src/hub/display.rs
/// Built-in source labels; falls back to the raw id for unknown sources.
#[must_use]
pub fn source_label(source_id: &str) -> &str {
    match source_id {
        "aleph-hub" => "Aleph Hub",
        "clawhub" => "ClawHub",
        "hermes-atlas" => "Hermes Atlas",
        "mcp-official" => "MCP Registry",
        "docker-mcp" => "Docker",
        "cc-marketplace" => "Plugin Marketplace",
        other => other,
    }
}
```

- [ ] **Step 2: Dedup + label in the handler**

In `catalog.rs`, after querying the cache into `Vec<ExtensionEntry>` and before serializing the response:
```rust
let order: Vec<String> =
    crate::hub::dedup::DEFAULT_HUB_PRIORITY.iter().map(|s| s.to_string()).collect();
let entries = crate::hub::dedup::dedup_by_priority(entries, &order);
```
When building each entry's JSON, add `"source_label": crate::hub::display::source_label(&e.source_id)`. Keep `repo_url` in the response (already serialized via `ExtensionEntry`). Prefer the live provider `display_name` when available (config hubs) — fall back to the static helper. (For v1 the static helper + raw id fallback is sufficient.)

- [ ] **Step 3: Compile-verify + commit**

Run: `cargo check -p alephcore --lib`

```bash
git add -A
git commit -m "hub: dedup catalog by priority + emit source_label provenance in extensions.catalog"
```

---

## Phase 5 — Background periodic sync

### Task 5.1: Periodic catalog sync task

**Files:** `src/bin/aleph-server/commands/start/mod.rs` (near the existing one-shot `tokio::spawn` sync at ~line 1378).

**Interfaces:**
- Consumes: `registry: Arc<ProviderRegistry>`, `cache: Arc<CatalogCache>`.
- Produces: a detached task that re-syncs every interval (no dependency on the retired store agent).

- [ ] **Step 1: Replace the one-shot spawn with initial + interval loop**

```rust
{
    let registry = registry.clone();
    let cache = cache.clone();
    tokio::spawn(async move {
        // Initial sync immediately, then every 6h.
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(6 * 60 * 60));
        loop {
            tick.tick().await;
            let report = registry.sync_all_into(&cache).await;
            tracing::info!(synced = ?report.synced, failed = ?report.failed, "extensions catalog sync");
        }
    });
}
```
(`interval` fires immediately on first `tick().await`, preserving the startup sync.)

- [ ] **Step 2: Confirm `hub_catalog_sync` tool still works for on-demand sync**

The `HubCatalogSyncTool` remains available to `main` (Phase 1) so the LLM can force a refresh on request; ensure its construction passes both `marketplaces` and the new `extension_hubs` (rebuild `build_default_registry(marketplaces, hubs)` inside the tool the same way as startup). Update `src/builtin_tools/hub/catalog_sync.rs` to also hold `hubs: Vec<ExtensionHubEntry>` and the constructor sites in the builder.

- [ ] **Step 3: Compile-verify + commit**

Run: `cargo check -p alephcore --lib`

```bash
git add -A
git commit -m "hub: background periodic catalog sync (replaces agent-driven refresh)"
```

---

## Phase 6 — Panel UI provenance (badge + View source)

### Task 6.1: Source badge + upstream link on cards/detail

**Files:** `interfaces/webchat/src/components/extensions/card.rs`, `detail_drawer.rs`, `labels.rs`; `interfaces/webchat/src/views/extensions/model.rs` (add `source_label` to the entry model); `interfaces/webchat/locales/{en,zh}.json`.

**Interfaces:**
- Consumes: `source_label` + `repo_url` from `extensions.catalog` (Phase 4).
- Produces: a `via {source_label}` badge and a `View source` link to `repo_url` on each card/detail.

- [ ] **Step 1: Extend the panel entry model**

In `views/extensions/model.rs`, add `pub source_label: String` (and ensure `repo_url: Option<String>` is present) to the deserialized entry; default to `source_id` if absent.

- [ ] **Step 2: Render badge + link**

In `card.rs` / `detail_drawer.rs`, render a small badge `format!("via {}", entry.source_label)` and, when `repo_url` is `Some`, an external link labelled by the i18n key `extensions.view_source` pointing to `repo_url` (open in new tab, `rel="noopener noreferrer"`).

- [ ] **Step 3: Localize**

Add to `locales/en.json`: `"extensions": { "view_source": "View source", "source_via": "via {name}" }` and the zh equivalents (`"查看源码"`, `"来自 {name}"`). Use the existing `leptos_i18n` pattern in the file.

- [ ] **Step 4: Rebuild WASM + verify panel builds**

Run: `just wasm` (or the project's WASM build) then `cargo check -p aleph-server` (panel is `rust_embed`-embedded at server compile — a fresh binary is needed to see it).
Expected: clean build.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "panel(extensions): source provenance badge + View source upstream link"
```

---

## Self-Review

**Spec coverage** (against `2026-06-20-extension-hub-federation-design.md` incl. addendum D1–D17):
- D1 hub-as-peer-source → Phase 2 (StaticHubProvider as a `SourceProvider`). ✅
- D2 static artifact form → Task 2.1/2.2 (artifact types + HTTP fetch). ✅
- D3 retire store agent, install via main-loop tools → Phase 1. ✅
- D6 install = operator tier → Task 1.3. ✅
- D9/D10 rename store→hub, surgical → Phase 0. ✅
- D11/§12.1 provenance (source label + repo_url) → Task 2.3, Phase 4, Phase 6. ✅
- D12 ClawHub generalized-now, adapter-later → no ClawHub provider built; labeling/priority include `clawhub` (display.rs, DEFAULT_HUB_PRIORITY). ✅
- D13 provenance UI in-scope → Phase 6. ✅
- D14 keep "Extensions" → no `extensions.*`/UI rename. ✅
- D15 config-driven hubs + builtin, no new RPC → Task 2.4. ✅
- D16 background periodic sync → Phase 5. ✅
- D17 cross-source dedup by repo, priority aleph>clawhub>hermes → Phase 3 + Task 4.1. ✅

**Placeholder scan:** `ALEPH_HUB_URL` is a deliberately-marked TODO constant (the real Aleph-Hub publish URL is produced by the separate Aleph-Hub project) — flagged, not silent. No other placeholders.

**Type consistency:** `HubCatalogEntry::into_entry(&self, hub_id)` (2.1) is consumed by `StaticHubProvider::ingest` (2.2). `display_name()` (2.3) is consumed by `list_sources` + handler. `dedup_by_priority` (3.1) is consumed by catalog handler (4.1). `ExtensionHubEntry` (2.4) is consumed by builder + `HubCatalogSyncTool` (5). Tool-name strings are `hub_*` consistently after Phase 0.

**Known follow-ups (out of scope, documented):** ClawHub/Hermes adapters; artifact signing; large-catalog sharding; monorepo dedup refinement; URL escape-hatch UI; "also available via X" multi-source provenance.
