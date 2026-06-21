# Hub Settings Sync & Nav Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move the MCP/Plugins/Skills settings tabs back into the "Extensions" nav group, and make the Hub browse grid show accurate installed-state by reconciling the cached catalog against the live installed set server-side.

**Architecture:** Two independent workstreams. WS1 is a pure nav-data edit in the Leptos panel (`settings_sidebar.rs`) plus its locking unit test. WS2 adds a server-side reconcile step inside the `extensions.catalog` RPC handler: a shared `collect_installed()` helper produces the live installed list (already used by `extensions.installed`), and a pure `mark_installed()` helper stamps `installed`/`enabled` onto each catalog entry — MCP matched exactly by its deterministic derived id, Plugin/Skill matched by case-insensitive name. The panel needs no struct changes.

**Tech Stack:** Rust (alephcore lib + aleph-server bin), Leptos/WASM panel (aleph-panel), JSON-RPC gateway.

## Global Constraints

- **Spec:** `docs/superpowers/specs/2026-06-21-hub-settings-sync-and-nav-design.md` (Approach A approved).
- **Tab order in the Extensions group:** `Mcp, Plugins, Skills, Acp` (verbatim, matches the `SettingsTab` enum declaration order).
- **MCP match identity (exact):** a catalog MCP entry is installed iff `format!("local:mcp:{}", mcp_server_id(&entry.id))` equals some installed entry id. `mcp_server_id(s)` = `s.replace([':', '/'], "_")` (already defined in `src/hub/install.rs:79`).
- **Plugin/Skill match identity:** case-insensitive `name` within the same `kind` (`ExtensionKind` is `Copy + Eq` but **not** `Hash` — key maps by `kind.as_str()` strings, never the enum).
- **No panel struct changes:** `interfaces/webchat/src/api/extensions.rs` already deserializes `installed`/`enabled`; `components/extensions/card.rs:67` already branches on `installed`.
- **Verification budget (repo discipline — `极度节制 cargo 调用`):** at most **one** `cargo check -p alephcore --lib` for WS2 and **one** `cargo check -p aleph-panel --target wasm32-unknown-unknown` for WS1. Do **not** run the full `cargo test -p alephcore` build locally — it is memory-heavy and OOMs (see memory `alephcore-build-memory`). Unit tests are authored for CI; if a local test run is unavoidable, scope it to the single module path shown and accept the build cost only once.
- **Commits:** English, `<scope>: <description>`. Work directly on `main` (single-branch repo). No attribution footer (disabled globally).
- **Runtime effect:** panel + handler changes only take effect after `just wasm` + server rebuild (`rust_embed` compile-time embed). Do that only in Task 3 (manual verification), and only when the user asks.

---

### Task 1: WS1 — Restore MCP/Plugins/Skills to the Extensions settings group

**Files:**
- Modify: `interfaces/webchat/src/components/settings_sidebar.rs` (the `SETTINGS_GROUPS` const, lines ~232-251, and the test at lines ~288-317)

**Interfaces:**
- Consumes: nothing (leaf data edit).
- Produces: nothing consumed by later tasks (independent workstream).

- [ ] **Step 1: Rewrite the locking test to assert the new (restored) state**

In `interfaces/webchat/src/components/settings_sidebar.rs`, replace the existing `mcp_plugins_skills_demoted_to_advanced` test (currently lines ~288-317) with:

```rust
    #[test]
    fn mcp_plugins_skills_in_extensions_group() {
        let extensions = group_tab_paths("Extensions");
        assert!(
            extensions.contains(&"/settings/mcp"),
            "Extensions must contain MCP"
        );
        assert!(
            extensions.contains(&"/settings/plugins"),
            "Extensions must contain Plugins"
        );
        assert!(
            extensions.contains(&"/settings/skills"),
            "Extensions must contain Skills"
        );

        let advanced = group_tab_paths("Advanced");
        assert!(
            !advanced.contains(&"/settings/mcp"),
            "Advanced must not contain MCP"
        );
        assert!(
            !advanced.contains(&"/settings/plugins"),
            "Advanced must not contain Plugins"
        );
        assert!(
            !advanced.contains(&"/settings/skills"),
            "Advanced must not contain Skills"
        );
    }
```

(Leave `clawhub_tab_is_removed` and the `all_tab_paths` / `group_tab_paths` helpers untouched.)

- [ ] **Step 2: Confirm the test now fails against the current const**

The const still has the three tabs in "Advanced", so the new assertions are false. This is the RED state. (No cargo run needed — it is a const-array membership check; the failure is evident by inspection. If running anyway, the host-target command is `cargo test -p aleph-panel mcp_plugins_skills_in_extensions_group`; if the panel crate does not build on host, skip and rely on Step 4's compile check.)

- [ ] **Step 3: Move the three tabs in `SETTINGS_GROUPS`**

In the same file, change the `"Extensions"` and `"Advanced"` group entries. Replace:

```rust
    SettingsGroup {
        label: "Extensions",
        tabs: &[SettingsTab::Acp],
    },
    SettingsGroup {
        label: "Advanced",
        tabs: &[
            SettingsTab::Browser,
            SettingsTab::Policies,
            SettingsTab::Security,
            SettingsTab::Execution,
            SettingsTab::Mcp,
            SettingsTab::Plugins,
            SettingsTab::Skills,
        ],
    },
```

with:

```rust
    SettingsGroup {
        label: "Extensions",
        tabs: &[
            SettingsTab::Mcp,
            SettingsTab::Plugins,
            SettingsTab::Skills,
            SettingsTab::Acp,
        ],
    },
    SettingsGroup {
        label: "Advanced",
        tabs: &[
            SettingsTab::Browser,
            SettingsTab::Policies,
            SettingsTab::Security,
            SettingsTab::Execution,
        ],
    },
```

- [ ] **Step 4: Compile-check the panel (the one WS1 cargo call)**

Run: `cargo check -p aleph-panel --target wasm32-unknown-unknown`
Expected: compiles clean. The restored test now passes by construction (Extensions contains the three paths; Advanced does not).

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/components/settings_sidebar.rs
git commit -m "panel: restore MCP/Plugins/Skills to the Extensions settings group"
```

---

### Task 2: WS2 — Reconcile installed-state into `extensions.catalog`

**Files:**
- Modify: `src/hub/install.rs:79` (make `mcp_server_id` `pub(crate)`)
- Modify: `src/gateway/handlers/extensions/catalog.rs` (add `collect_installed`, `mark_installed`; new `handle_catalog` signature + reconcile; thin `handle_installed`; tests)
- Modify: `src/bin/aleph-server/commands/start/builder/handlers/extensions.rs:20-28` (thread `mcp` into the `extensions.catalog` registration)

**Interfaces:**
- Consumes: `mcp_to_entry` / `plugin_to_entry` / `skill_to_entry` (`src/hub/reconcile.rs`), `mcp_server_id` (`src/hub/install.rs`), `McpManagerHandle::list_servers`, `crate::extension::try_extension_manager`, `shared_system().full_status()`.
- Produces:
  - `pub async fn collect_installed(mcp: Option<McpManagerHandle>) -> Vec<ExtensionEntry>`
  - `pub async fn handle_catalog(req: JsonRpcRequest, cache: Arc<CatalogCache>, mcp: Option<McpManagerHandle>) -> JsonRpcResponse`
  - `pub async fn handle_installed(req: JsonRpcRequest, mcp: Option<McpManagerHandle>) -> JsonRpcResponse` (unchanged signature)
  - private `fn mark_installed(catalog: &mut [ExtensionEntry], installed: &[ExtensionEntry])`

- [ ] **Step 1: Make `mcp_server_id` reusable**

In `src/hub/install.rs`, change line 79 from:

```rust
fn mcp_server_id(entry_id: &str) -> String {
```

to:

```rust
pub(crate) fn mcp_server_id(entry_id: &str) -> String {
```

(Body unchanged: `entry_id.replace([':', '/'], "_")`.)

- [ ] **Step 2: Write the failing unit tests for `mark_installed`**

In `src/gateway/handlers/extensions/catalog.rs`, append this test module at the end of the file:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub::types::{ExtensionCategory, ExtensionEntry, ExtensionKind, TrustTier};

    fn catalog_entry(id: &str, kind: ExtensionKind, name: &str) -> ExtensionEntry {
        ExtensionEntry {
            id: id.into(),
            kind,
            category: ExtensionCategory::Other,
            name: name.into(),
            description: String::new(),
            author: None,
            icon: None,
            tags: vec![],
            version: None,
            source_id: "aleph-hub".into(),
            repo_url: None,
            trust_tier: TrustTier::Unverified,
            requires_config: false,
            config_schema: None,
            installed: false,
            enabled: false,
            update_available: false,
            via: Some("Aleph Hub".into()),
            install_spec: None,
        }
    }

    fn installed_entry(id: &str, kind: ExtensionKind, name: &str, enabled: bool) -> ExtensionEntry {
        let mut e = catalog_entry(id, kind, name);
        e.installed = true;
        e.enabled = enabled;
        e.source_id = "local".into();
        e.via = None;
        e
    }

    #[test]
    fn mcp_entry_marked_installed_by_derived_id() {
        // catalog id "aleph-hub:github" -> install id "aleph-hub_github"
        // -> reconciled installed id "local:mcp:aleph-hub_github"
        let mut catalog = vec![catalog_entry("aleph-hub:github", ExtensionKind::Mcp, "GitHub")];
        let installed = vec![installed_entry(
            "local:mcp:aleph-hub_github",
            ExtensionKind::Mcp,
            "GitHub",
            true,
        )];
        mark_installed(&mut catalog, &installed);
        assert!(catalog[0].installed);
        assert!(catalog[0].enabled);
    }

    #[test]
    fn mcp_entry_not_installed_when_no_match() {
        let mut catalog = vec![catalog_entry("aleph-hub:absent", ExtensionKind::Mcp, "Nope")];
        let installed = vec![installed_entry(
            "local:mcp:something-else",
            ExtensionKind::Mcp,
            "Other",
            true,
        )];
        mark_installed(&mut catalog, &installed);
        assert!(!catalog[0].installed);
    }

    #[test]
    fn plugin_entry_marked_installed_by_name_case_insensitive() {
        let mut catalog = vec![catalog_entry(
            "aleph-hub:cool-plugin",
            ExtensionKind::Plugin,
            "Cool Plugin",
        )];
        // discovered plugin id differs; matched by name; enabled=false propagates
        let installed = vec![installed_entry(
            "local:plugin:whatever",
            ExtensionKind::Plugin,
            "cool plugin",
            false,
        )];
        mark_installed(&mut catalog, &installed);
        assert!(catalog[0].installed);
        assert!(!catalog[0].enabled);
    }

    #[test]
    fn name_match_does_not_cross_kinds() {
        let mut catalog = vec![catalog_entry("aleph-hub:x", ExtensionKind::Skill, "Shared Name")];
        let installed = vec![installed_entry(
            "local:plugin:x",
            ExtensionKind::Plugin,
            "Shared Name",
            true,
        )];
        mark_installed(&mut catalog, &installed);
        assert!(!catalog[0].installed);
    }
}
```

- [ ] **Step 3: Confirm the tests fail to compile (RED)**

`mark_installed` does not exist yet, so the module will not compile. That is the expected RED state. (Do not spend a cargo run here — the missing symbol is evident. The single compile check happens in Step 7.)

- [ ] **Step 4: Add `mark_installed` and `collect_installed`; rewrite the handlers**

In `src/gateway/handlers/extensions/catalog.rs`, update the imports and the handler bodies. Replace the existing import block (lines 4-13) and both handler functions (lines 23-88) with:

```rust
use crate::gateway::handlers::parse_params;
use crate::gateway::handlers::skills::shared_system;
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR};
use crate::hub::cache::{CatalogCache, CatalogFilter};
use crate::hub::install::mcp_server_id;
use crate::hub::reconcile::{mcp_to_entry, plugin_to_entry, skill_to_entry};
use crate::hub::types::{ExtensionCategory, ExtensionEntry, ExtensionKind};
use crate::mcp::manager::McpManagerHandle;
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, Default, Deserialize)]
pub struct CatalogParams {
    pub kind: Option<ExtensionKind>,
    pub category: Option<ExtensionCategory>,
    pub source_id: Option<String>,
    pub query: Option<String>,
}

/// Live-reconciled installed extensions across MCP / plugins / skills.
///
/// Best-effort: a failing or empty backend is logged and skipped — it never
/// aborts, so a flaky MCP actor cannot blank the catalog or installed views.
/// All calls are local (no network), so callers stay offline-capable.
pub async fn collect_installed(mcp: Option<McpManagerHandle>) -> Vec<ExtensionEntry> {
    let mut out = Vec::new();

    if let Some(mcp) = &mcp {
        match mcp.list_servers().await {
            Ok(servers) => out.extend(servers.iter().map(mcp_to_entry)),
            Err(e) => tracing::warn!("collect_installed: mcp list failed: {e}"),
        }
    }

    if let Some(mgr) = crate::extension::try_extension_manager() {
        if let Err(e) = mgr.ensure_loaded().await {
            tracing::warn!("collect_installed: failed to load plugins: {e}");
        }
        out.extend(mgr.list_plugin_records().await.iter().map(plugin_to_entry));
    }

    out.extend(shared_system().full_status().await.iter().map(skill_to_entry));

    out
}

/// Stamp `installed` / `enabled` onto each catalog entry by matching it against
/// the live installed set. MCP matches exactly by its deterministic derived id
/// (`local:mcp:{mcp_server_id(entry.id)}`); Plugin / Skill match by
/// case-insensitive `name` within the same `kind`.
fn mark_installed(catalog: &mut [ExtensionEntry], installed: &[ExtensionEntry]) {
    // (kind.as_str(), lowercased name) -> enabled, for Plugin/Skill matching.
    let by_name: HashMap<(String, String), bool> = installed
        .iter()
        .map(|e| {
            (
                (e.kind.as_str().to_string(), e.name.trim().to_lowercase()),
                e.enabled,
            )
        })
        .collect();

    for e in catalog.iter_mut() {
        let enabled = if e.kind == ExtensionKind::Mcp {
            let expected = format!("local:mcp:{}", mcp_server_id(&e.id));
            installed
                .iter()
                .find(|ie| ie.id == expected)
                .map(|ie| ie.enabled)
        } else {
            by_name
                .get(&(e.kind.as_str().to_string(), e.name.trim().to_lowercase()))
                .copied()
        };
        if let Some(en) = enabled {
            e.installed = true;
            e.enabled = en;
        }
    }
}

/// extensions.catalog — filtered read of the cached catalog, reconciled against
/// the live installed set so browse cards show accurate installed-state.
pub async fn handle_catalog(
    req: JsonRpcRequest,
    cache: Arc<CatalogCache>,
    mcp: Option<McpManagerHandle>,
) -> JsonRpcResponse {
    let p: CatalogParams = if req.params.is_some() {
        match parse_params(&req) {
            Ok(p) => p,
            Err(e) => return e,
        }
    } else {
        CatalogParams::default()
    };
    let filter = CatalogFilter {
        kind: p.kind,
        category: p.category,
        source_id: p.source_id,
        query: p.query,
        ..Default::default()
    };
    match cache.query(&filter).await {
        Ok(mut entries) => {
            let installed = collect_installed(mcp).await;
            mark_installed(&mut entries, &installed);
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
        Err(e) => JsonRpcResponse::error(req.id, INTERNAL_ERROR, e.to_string()),
    }
}

/// extensions.installed — live reconciled list across all backends.
pub async fn handle_installed(req: JsonRpcRequest, mcp: Option<McpManagerHandle>) -> JsonRpcResponse {
    let out = collect_installed(mcp).await;
    JsonRpcResponse::success(req.id, json!({ "extensions": out }))
}
```

- [ ] **Step 5: Thread `mcp` into the `extensions.catalog` registration**

In `src/bin/aleph-server/commands/start/builder/handlers/extensions.rs`, replace the `extensions.catalog` block (lines 20-28):

```rust
    {
        let cache = cache.clone();
        server
            .handlers_mut()
            .register("extensions.catalog", move |req| {
                let cache = cache.clone();
                async move { extensions::catalog::handle_catalog(req, cache).await }
            });
    }
```

with:

```rust
    {
        let cache = cache.clone();
        let mcp = mcp.clone();
        server
            .handlers_mut()
            .register("extensions.catalog", move |req| {
                let cache = cache.clone();
                let mcp = mcp.clone();
                async move { extensions::catalog::handle_catalog(req, cache, mcp).await }
            });
    }
```

(The outer `mcp` param stays available for the `extensions.installed` / `toggle` / `uninstall` blocks below, which each do their own `let mcp = mcp.clone();`.)

- [ ] **Step 6: Check for other `handle_catalog` callers**

Run: `git grep -n "catalog::handle_catalog\|extensions::catalog::handle_catalog"`
Expected: only the one registration site edited in Step 5. (The `providers::handle_catalog` / `tools_visibility::handle_catalog` matches are unrelated namespaces — ignore them.) If any other caller of `extensions::catalog::handle_catalog` exists, add the `mcp` argument there too.

- [ ] **Step 7: Compile-check the lib (the one WS2 cargo call)**

Run: `cargo check -p alephcore --lib`
Expected: compiles clean. This covers `mcp_server_id` visibility, the new `handle_catalog` signature, `mark_installed`/`collect_installed`, and the rewritten `handle_installed`.

If you must also execute the new unit tests, scope to exactly: `cargo test -p alephcore --lib gateway::handlers::extensions::catalog::tests` (single module; accept the one-time build cost — do not run the whole suite).

- [ ] **Step 8: Verify the bin compiles (registration site)**

Run: `cargo check -p alephcore --bin aleph-server` *only if* Step 7 did not already cover the bin. (Step 7 checks the lib; the registration edit lives in the `aleph-server` bin, so this second check confirms the call-site signature. If staying within the one-cargo-call budget, prefer `cargo check --bin aleph-server` here since it builds lib+bin together and supersedes Step 7.)

- [ ] **Step 9: Commit**

```bash
git add src/hub/install.rs src/gateway/handlers/extensions/catalog.rs src/bin/aleph-server/commands/start/builder/handlers/extensions.rs
git commit -m "hub: reconcile installed-state into extensions.catalog browse view"
```

---

### Task 3: Runtime end-to-end verification (manual — only when the user asks)

**Files:** none (build + observe).

**Interfaces:**
- Consumes: Tasks 1 & 2 merged.
- Produces: a verification result (pass/fail observations).

> This task rebuilds and runs the desktop/server stack. Per the repo's cargo discipline and the `rust_embed` compile-time panel embed, do this only when explicitly requested.

- [ ] **Step 1: Rebuild panel + server**

Run: `just wasm` then rebuild/replace the running `aleph-server` binary (see `docs/reference/DESKTOP_SHELL.md` for the dev / macOS / Windows daemon-replacement procedure).

- [ ] **Step 2: Nav check (WS1)**

Open Settings. Confirm the sidebar shows **Extensions → MCP, Plugins, Skills, ACP** (in that order) and that "Advanced" no longer lists MCP/Plugins/Skills.

- [ ] **Step 3: Install round-trip (WS2)**

From the Aleph Hub browse grid, install one MCP server and one plugin. Confirm:
- each appears on its settings page (Settings → Extensions → MCP / Plugins), and
- its Hub **browse card** now shows the `Installed` badge (previously always absent).

- [ ] **Step 4: Removal round-trip (WS2)**

Delete the MCP server from the MCP settings page and uninstall the plugin from the Plugins settings page. Re-open the Hub browse grid and confirm both cards flip back to not-installed, and the "Installed" slide-in panel no longer lists them.

- [ ] **Step 5: Record the result**

Note pass/fail per step in the PR/commit description or back to the user. No commit (observation only).

---

## Self-Review

**1. Spec coverage:**
- WS1 nav move → Task 1. ✓ (group membership + order + inverted test)
- WS2 `collect_installed` extraction → Task 2 Step 4. ✓
- WS2 `mcp_server_id` pub(crate) → Task 2 Step 1. ✓
- WS2 reconcile in `handle_catalog` + new signature → Task 2 Step 4. ✓
- WS2 registration wiring → Task 2 Step 5. ✓
- Tests (MCP exact / MCP miss / Plugin name / cross-kind) → Task 2 Step 2. ✓
- Runtime e2e → Task 3. ✓
- Non-goals (no version-diff, no Approach C, no install-flow change, no panel struct change) → respected; not implemented. ✓

**2. Placeholder scan:** No TBD/TODO/"handle edge cases"/"similar to". All code blocks are complete. ✓

**3. Type consistency:** `mcp_server_id` (defined Task 2 Step 1, used Step 4); `collect_installed` / `mark_installed` / `handle_catalog` signatures match between the Produces block, Step 4 code, and Step 5 call-site (`handle_catalog(req, cache, mcp)`). `ExtensionEntry` 19-field literal matches `src/hub/types.rs`. `ExtensionKind::as_str()` used (exists, per `reconcile.rs`). Map keyed by `kind.as_str()` strings (enum is not `Hash`). ✓
