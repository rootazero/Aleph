# Extensions Store P4 — Store Agent Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Register a protected, non-deletable built-in `store` agent with a private `STORE_TOOLS` tool set (5 scaffolded `AlephTool` builtins), generalize the agent-deletion guard to all built-ins, and add deterministic functional-category assignment so the catalog browse (P3) is populated instead of collapsing to `Other`.

**Architecture:** P4 lands the Store Agent *infrastructure and the one deterministic curation win*. The 5 store tools are built as real `AlephTool` builtins (the agent can only call `AlephTool::NAME` tools, never `extensions.*` JSON-RPC). Curation in v1 is a deterministic keyword→category mapper applied as a post-sync enrichment pass (no LLM). The long-tail URL/LLM install path is *scaffolded but not wired* to any user surface; the supported install path stays P3's UI → `extensions.install`.

**Tech Stack:** Rust (alephcore crate), `async_trait`, `schemars` (tool arg schemas), `serde`, `rusqlite` (catalog cache), tokio. No frontend changes in P4.

**Authored:** 2026-06-20 (filename dated 2026-06-19 for sibling consistency with the plan family). **Base before Task 1:** `1f1d3dd2b` (P3 head). **Branch:** `feat/unified-extensions-store`.

---

## Global Constraints

Every task implicitly includes these. Values copied verbatim from the spec (`docs/superpowers/specs/2026-06-19-unified-extensions-store-design.md` §9–§11) and corrected against the live interfaces verified 2026-06-20.

- **The 5 store-tool NAMEs are fixed and are the single source of truth** (used identically by `STORE_TOOLS`, by each tool's `AlephTool::NAME`, and by the chat-agent deny list): `store_catalog_sync`, `store_fetch_docs`, `store_resolve_spec`, `store_install_run`, `store_install_verify`.
- **Trust rails are system-enforced, never agent-discretionary.** `store_install_run` MUST build the disclosure (`store::trust::build_disclosure`), honor the ack gate, and route through the deterministic `store::install::run_install`. The agent **cannot self-ack**: a real install only proceeds when an explicit `ack=true` flows in as a tool argument representing user consent. OCI installs are rejected (`run_install` already returns `Err`).
- **Reuse, do not fork:** `store::provider::{build_default_registry, ProviderRegistry::sync_all_into, SourceProvider::resolve_install_spec}`, `store::install::run_install`, `store::trust::{build_disclosure, scan_for_injection}`, `store::secrets::{field_key, secret_ref}`, `store::cache::CatalogCache`, the agent subsystem (`builtin_agents()`, `AgentDef` builder, `tool_sets::resolve`, `AllowlistToolService`), the `AlephTool` builtin pipeline.
- **Curation is deterministic-only in v1** (user decision 2026-06-20). No LLM enrichment loop. Category assignment is a pure keyword map; LLM-for-ambiguous-entries is a documented fast-follow, not built.
- **Long-tail (URL/LLM install) is scaffolded, not wired** (user decision 2026-06-20). The 5 tools (incl. `store_fetch_docs`/`store_resolve_spec`) are implemented and unit-tested, but no user-facing long-tail install surface is added. The deterministic fast-path install (P2/P3) remains the supported path.
- **`store` agent is `AgentMode::SubAgent`, `AgentSource::Builtin`** (Builtin is the `AgentDef::new` default — no setter needed). A SubAgent cannot itself spawn subagents (the `subagent` tool is unconditionally denied for SubAgent mode — `types.rs:277`); this is acceptable for v1.
- **Repo conventions:** `docs/` is gitignored — this plan is force-added (`git add -f`). Handlers return `JsonRpcResponse`; internal ops return `Result<T, String>` or typed errors. Memory-heavy build: scope tests narrowly (`cargo test -p alephcore --lib store::<module>` / `agents::<module>`). `aleph-server` is a **bin target** of `alephcore` (`cargo build --bin aleph-server`, not `-p aleph-server`). The pre-existing-broken `tests/cancellation_chain.rs` means full `cargo test -p alephcore` fails — gate on `--lib`.

---

## Finding-driven deviations from the spec (resolved here; surfaced for review)

The interface verification (2026-06-20) corrected five spec assumptions. Each is resolved in-plan:

1. **"Store tools wrap `extensions.*` RPC" → they must be `AlephTool` builtins.** The `store` agent can only call tools whose name is an `AlephTool::NAME`; `extensions.*` are gateway JSON-RPC methods, a separate pipeline. P4 builds 5 new builtins, each wired at the verified 5 sites (definitions catalog, factory/with_config construction, `execute_tool` dispatch, `register_core_tools` schema, registry struct field).
2. **"`allowed_tool_sets=["STORE_TOOLS"]` keeps tools from chat agents" → false; wildcard defeats it.** `main` has `allowed_tools:["*","flow_run"]` and `verify` has `["*"]` — the wildcard overrides tool-set scoping (`is_tool_allowed` priority: denied_tools beats wildcard). **Resolution:** add the 5 store-tool NAMEs to `denied_tools` of `main` and `verify` (Task 4). `denied_tools` short-circuits before the wildcard, so this closes the hole. *Minor (documented):* any future wildcard agent must also deny them — a latent footgun until the architecture moves to deny-by-default.
3. **"Hide delete/disable in the Agents panel" → the store agent isn't in the panel.** `agents.list` → `AgentManager::list()` → TOML `config.list` only (`crud.rs:182`); `builtin_agents()` is a separate subagent catalog never merged into it. The store builtin shows no Delete button to hide. **Resolution:** no panel task; the generalized delete-tool guard (Task 4) is the protection. (Also: there is no agent disable control anywhere — not built.)
4. **"Background curation (editorial/high-star)" → infeasible in v1.** No `featured`/`stars` field on `ExtensionEntry`, no such column, no provider exposes star counts (only the deferred clawhub does), and no background scheduler exists (`MemoryProducerScheduler` is memory-only and can't spawn agents). P3 already derives "featured" client-side from trust tier (`featured_picks`). **Resolution:** v1 curation = deterministic categories only (Task 1). Featured/high-star/editorial and a periodic scheduler are deferred (documented), not built.
5. **SHA256 not verified for plugin installs.** `run_install`'s GitDir arm calls `marketplace.install_to_scope(..., None)` (`install.rs:114`) — the pinned `sha256` from `InstallSpec::GitDir` is shown in the disclosure but not passed to the integrity check. `verify_plugin_integrity(path, Some(&sha))` exists (`installer.rs:186`) and works. **Resolution:** Task 7 audits `install_to_scope` and threads the spec's `sha256` through if the marketplace path doesn't already enforce it; if it's a real gap it is a P2 regression and is fixed here (security-critical).

---

## Whole-phase file map

### New (backend)
| File | Responsibility | Task |
|---|---|---|
| `src/store/categorize.rs` | pure deterministic `categorize()` + `category_from_hint()` keyword maps | T1 |
| `src/store/verify.rs` | post-install verification (`verify_install` + pure `verdict`) | T2 |
| `src/builtin_tools/store/mod.rs` | module root + re-exports for the 5 store tools | T5 |
| `src/builtin_tools/store/catalog_sync.rs` | `StoreCatalogSyncTool` (`store_catalog_sync`) | T5 |
| `src/builtin_tools/store/resolve_spec.rs` | `StoreResolveSpecTool` (`store_resolve_spec`) | T6 |
| `src/builtin_tools/store/fetch_docs.rs` | `StoreFetchDocsTool` (`store_fetch_docs`, scaffold) | T6 |
| `src/builtin_tools/store/install_run.rs` | `StoreInstallRunTool` (`store_install_run`, trust-gated) | T7 |
| `src/builtin_tools/store/install_verify.rs` | `StoreInstallVerifyTool` (`store_install_verify`) | T8 |

### Modified (backend)
- `src/agents/tool_sets.rs` — add `STORE_TOOLS` const + `resolve()` arm (T3)
- `src/agents/registry.rs` — add `store` builtin in `builtin_agents()`, alias in `normalize_agent_alias()`, bump count test 7→8 (T3); add `denied_tools` to `main` + `verify` (T4)
- `src/builtin_tools/agent_manage/delete.rs` — generalize guard to `AgentSource::Builtin` + inject `Arc<crate::agents::AgentRegistry>` (T4)
- `src/store/mod.rs` — `pub mod categorize;` (T1), `pub mod verify;` (T2)
- `src/store/provider/mod.rs` — categorize post-pass in `sync_all_into`; add `resolve_for_entry` helper (T1, T6)
- `src/store/provider/docker_mcp.rs` — map upstream `category` hint via `category_from_hint` (T1)
- `src/builtin_tools/mod.rs` — `pub mod store;` + re-exports (T5)
- `src/executor/builtin_registry/definitions.rs` — `BUILTIN_TOOL_DEFINITIONS` entries + `create_tool_boxed`/with_config (T5–T8)
- `src/executor/builtin_registry/builder/constructor/mod.rs` — construct store tools w/ injected deps (T5–T8)
- `src/executor/builtin_registry/builder/core_tools.rs` — `register_core_tools` schema entries (T5–T8)
- `src/executor/builtin_registry/registry/tool_registry_impl.rs` — `execute_tool` dispatch arms (T5–T8)
- `src/executor/builtin_registry/registry/struct_def.rs` (or equivalent struct) — store-tool fields (T5–T8)

### Explicitly NOT in P4 (deferred, documented)
LLM curation loop · long-tail user install surface · `featured`/`stars` columns + editorial ranking · periodic curation scheduler · Agents-panel changes · agent disable control · OCI/Docker container install (already `Err`).

---

## Tool wiring checklist (applies to every store tool, T5–T8)

Verified sites a new `AlephTool` builtin must touch (from interface findings). The implementer follows this for each tool; T5 establishes the pattern, T6–T8 repeat it:

1. **Struct + `impl AlephTool`** in `src/builtin_tools/store/<tool>.rs` (`#[derive(Clone)]`, `#[async_trait]`, `NAME`/`DESCRIPTION`/`Args`/`Output`/`call`). `Args` derives `serde::{Serialize,Deserialize}` + `schemars::JsonSchema`; `Output` derives `serde::Serialize`. Domain errors via `crate::error::AlephError::tool(msg)`.
2. **`BUILTIN_TOOL_DEFINITIONS`** entry in `definitions.rs` (name, description, `requires_config`).
3. **Construction** in `BuiltinToolRegistry::with_config` (`builder/constructor/mod.rs`) — store tools need runtime deps (catalog cache, provider registry inputs, MCP/marketplace handles, vault token manager); construct there and assign to a struct field. (Tools without runtime deps could use `create_tool_boxed`, but all store tools have deps → `with_config`.)
4. **Struct field** on `BuiltinToolRegistry` (`Option<TheTool>` if conditionally available).
5. **`execute_tool` dispatch arm** in `registry/tool_registry_impl.rs`: `"store_xxx" => Box::pin(async move { self.store_xxx.as_ref().ok_or_else(|| AlephError::tool("store_xxx not configured"))?.call_json(arguments).await })`.
6. **`register_core_tools`** entry in `core_tools.rs` (inserts the `UnifiedTool` schema via `schemars::schema_for!(Args)`), else the model sees an empty parameter schema.

> **Implementer-verify (T5, the integration seam):** read `BuiltinToolConfig` (the arg to `with_config`) and confirm which store handles are available there. P0–P2 wired the `CatalogCache` / MCP / marketplace handles into the **gateway handler registry** (`src/bin/aleph-server/commands/start/builder/handlers/`), NOT necessarily into `BuiltinToolConfig`. If absent, thread them in following the same construction the start builder uses (`build_default_registry(marketplace_configs)`, `CatalogCache::open(~/.aleph/store_catalog.db)`, the `McpManagerHandle`, `MarketplaceManager`, `SharedTokenManager` vault). Share one `CatalogCache`/registry construction with the gateway handlers where practical; reconstructing a second cache over the same DB file is acceptable (rusqlite, same path) but note it.

---

## Task 1: Deterministic category mapper

**Files:**
- Create: `src/store/categorize.rs`
- Modify: `src/store/mod.rs` (add `pub mod categorize;`)
- Modify: `src/store/provider/docker_mcp.rs` (use the upstream `category` hint)
- Modify: `src/store/provider/mod.rs` (post-sync categorize pass in `sync_all_into`)

**Interfaces:**
- Consumes: `crate::store::types::ExtensionCategory` (13 variants incl. `Other`), `ExtensionEntry` (`name`, `description`, `tags`, `category`), `ProviderRegistry::sync_all_into` (`provider/mod.rs:86`), `docker_server_to_extension` (`docker_mcp.rs:33`) + `DockerServer::category: Option<String>` (`docker_mcp.rs:24`).
- Produces: `pub fn categorize(name: &str, description: &str, tags: &[String], hint: Option<&str>) -> ExtensionCategory`; `pub fn category_from_hint(hint: &str) -> Option<ExtensionCategory>`.

- [ ] **Step 1: Write the failing tests** (`src/store/categorize.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::types::ExtensionCategory as C;

    #[test]
    fn hint_maps_known_upstream_categories() {
        assert_eq!(category_from_hint("developer"), Some(C::Developer));
        assert_eq!(category_from_hint("Database"), Some(C::Data));
        assert_eq!(category_from_hint("search"), Some(C::Search));
        assert_eq!(category_from_hint("nonsense"), None);
    }

    #[test]
    fn hint_wins_over_text() {
        // text says "github" (Developer) but explicit hint says data
        let c = categorize("gh thing", "github helper", &[], Some("data"));
        assert_eq!(c, C::Data);
    }

    #[test]
    fn text_keywords_route_to_category() {
        assert_eq!(categorize("pg", "a postgres database client", &[], None), C::Data);
        assert_eq!(categorize("ghx", "github pull request tool", &[], None), C::Developer);
        assert_eq!(categorize("brave", "web search via brave", &[], None), C::Search);
        assert_eq!(categorize("slackbot", "post to slack channels", &[], None), C::Communication);
    }

    #[test]
    fn unknown_text_is_other() {
        assert_eq!(categorize("zzz", "an inscrutable widget", &[], None), C::Other);
    }

    #[test]
    fn tags_are_considered() {
        assert_eq!(categorize("x", "no hints in name", &["database".into()], None), C::Data);
    }
}
```

- [ ] **Step 2: Run → FAIL** `cargo test -p alephcore --lib store::categorize::tests` (Expected: module/fn not found)

- [ ] **Step 3: Implement** `src/store/categorize.rs`

```rust
//! Deterministic functional-category assignment for catalog entries.
//!
//! No LLM (v1 decision): a keyword map over name+description+tags, with an
//! optional upstream hint (e.g. the Docker MCP catalog's `category`) taking
//! precedence. Runs as a post-sync enrichment pass so the panel's category
//! browse (P3) is populated instead of every entry collapsing to `Other`.
use crate::store::types::ExtensionCategory;

/// Map a raw upstream category string to our enum. `None` when unrecognized.
#[must_use]
pub fn category_from_hint(hint: &str) -> Option<ExtensionCategory> {
    use ExtensionCategory::{
        Automation, Communication, Data, Design, Developer, Files, Finance, Knowledge,
        Productivity, Search, Utilities, Writing,
    };
    Some(match hint.trim().to_ascii_lowercase().as_str() {
        "search" | "web-search" | "web_search" => Search,
        "developer" | "dev" | "development" | "devops" | "ci-cd" | "ci/cd" => Developer,
        "data" | "database" | "databases" | "analytics" => Data,
        "productivity" => Productivity,
        "writing" => Writing,
        "communication" | "messaging" | "chat" | "email" => Communication,
        "knowledge" | "docs" | "documentation" | "reference" => Knowledge,
        "files" | "storage" | "filesystem" => Files,
        "design" => Design,
        "automation" | "workflow" => Automation,
        "finance" | "payments" | "crypto" => Finance,
        "utilities" | "utility" | "tools" => Utilities,
        _ => return None,
    })
}

/// Keyword groups, most specific first. First group with any keyword present
/// in the haystack wins; otherwise `Other`.
const GROUPS: &[(&[&str], ExtensionCategory)] = &[
    (
        &["postgres", "mysql", "sqlite", "mongodb", "database", " sql", "bigquery", "snowflake", "redis", "duckdb"],
        ExtensionCategory::Data,
    ),
    (
        &["web search", "brave search", "google search", "serp", "duckduckgo", "perplexity", "websearch"],
        ExtensionCategory::Search,
    ),
    (
        &["github", "gitlab", "kubernetes", "docker", "terraform", "jira", "compiler", "debugger", "lint", "devops"],
        ExtensionCategory::Developer,
    ),
    (
        &["slack", "discord", "telegram", "gmail", "sendgrid", "twilio", " sms", "mailgun"],
        ExtensionCategory::Communication,
    ),
    (
        &["notion", "obsidian", "confluence", "wiki", "knowledge base"],
        ExtensionCategory::Knowledge,
    ),
    (
        &["filesystem", "file system", " s3", "dropbox", "google drive", "ftp", "object storage"],
        ExtensionCategory::Files,
    ),
    (&["figma", "canva", "image generation", "design"], ExtensionCategory::Design),
    (
        &["stripe", "paypal", "payment", "invoice", "accounting", "ethereum", "finance"],
        ExtensionCategory::Finance,
    ),
    (&["calendar", "todo", "reminder", "productivity"], ExtensionCategory::Productivity),
    (&["grammar", "copywriting", "blog post", "writing assistant"], ExtensionCategory::Writing),
    (&["zapier", "automation", "cron", "scheduler", "workflow"], ExtensionCategory::Automation),
];

/// Deterministic category from free text. Hint (if recognized) wins.
#[must_use]
pub fn categorize(
    name: &str,
    description: &str,
    tags: &[String],
    hint: Option<&str>,
) -> ExtensionCategory {
    if let Some(c) = hint.and_then(category_from_hint) {
        return c;
    }
    let hay = format!("{name} {description} {}", tags.join(" ")).to_ascii_lowercase();
    for (keys, cat) in GROUPS {
        if keys.iter().any(|k| hay.contains(k)) {
            return *cat;
        }
    }
    ExtensionCategory::Other
}
```

- [ ] **Step 4: Run → PASS** `cargo test -p alephcore --lib store::categorize::tests`. Add `pub mod categorize;` to `src/store/mod.rs`.

- [ ] **Step 5: Use the Docker upstream hint.** In `src/store/provider/docker_mcp.rs::docker_server_to_extension` (line 33), replace the hardcoded `category: ExtensionCategory::Other` with:
```rust
category: s.category
    .as_deref()
    .and_then(crate::store::categorize::category_from_hint)
    .unwrap_or(crate::store::categorize::categorize(name, &description, &tags, None)),
```
(Use the locals as they are named in that fn; if `description`/`tags` are built later, move the `category` assignment after them, or inline the field values.) **Implementer-verify** the exact local names in that function.

- [ ] **Step 6: Post-sync categorize pass.** In `src/store/provider/mod.rs::sync_all_into` (line 86), before each provider's `cache.replace_source(source_id, &entries)`, re-categorize entries still `Other`:
```rust
for e in &mut entries {
    if e.category == crate::store::types::ExtensionCategory::Other {
        e.category = crate::store::categorize::categorize(&e.name, &e.description, &e.tags, None);
    }
}
```
**Implementer-verify** the loop variable holding each provider's `Vec<ExtensionEntry>` is mutable at that point (clone into a `mut` binding if `sync_all_into` currently holds it immutably). This is the single always-run site (covers boot sync, `extensions.sources.refresh`, and the `store_catalog_sync` tool).

- [ ] **Step 7: Build gate** `cargo build --bin aleph-server` — clean.

- [ ] **Step 8: Commit** `feat(store): deterministic functional-category mapper + post-sync enrichment`

---

## Task 2: Post-install verification

**Files:**
- Create: `src/store/verify.rs`
- Modify: `src/store/mod.rs` (add `pub mod verify;`)

**Interfaces:**
- Consumes: `crate::store::install::InstallOutcome` (`Mcp { id }` / `Plugin { path }`), the MCP manager health type. Per findings, MCP health lives in `crate::mcp::manager::{HealthStatus, McpServerInfo}` and `McpManagerHandle`.
- Produces: `pub struct VerifyReport { pub ok: bool, pub detail: String }`; pure `pub fn verdict(running: bool, tool_count: usize) -> VerifyReport`; `pub async fn verify_install(outcome: &InstallOutcome, mcp: Option<&McpManagerHandle>) -> VerifyReport`.

- [ ] **Step 1: Write the failing tests** (`src/store/verify.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn running_with_tools_is_ok() {
        let r = verdict(true, 3);
        assert!(r.ok);
        assert!(r.detail.contains('3'));
    }

    #[test]
    fn running_without_tools_is_warn() {
        let r = verdict(true, 0);
        assert!(!r.ok);
    }

    #[test]
    fn not_running_is_fail() {
        let r = verdict(false, 0);
        assert!(!r.ok);
    }
}
```

- [ ] **Step 2: Run → FAIL** `cargo test -p alephcore --lib store::verify::tests`

- [ ] **Step 3: Implement** `src/store/verify.rs`

```rust
//! Post-install verification (spec §10). MCP: started + lists ≥1 tool.
//! Plugin: artifact present on disk. Honest report — never silent "success".
use crate::store::install::InstallOutcome;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct VerifyReport {
    pub ok: bool,
    pub detail: String,
}

/// Pure verdict from an MCP server's observed state.
#[must_use]
pub fn verdict(running: bool, tool_count: usize) -> VerifyReport {
    match (running, tool_count) {
        (true, n) if n > 0 => VerifyReport { ok: true, detail: format!("running; {n} tools") },
        (true, 0) => VerifyReport { ok: false, detail: "running but exposes 0 tools".into() },
        (false, _) => VerifyReport { ok: false, detail: "server not running".into() },
    }
}

/// Verify an install outcome. MCP uses the manager handle; plugin checks disk.
pub async fn verify_install(
    outcome: &InstallOutcome,
    mcp: Option<&crate::mcp::manager::McpManagerHandle>,
) -> VerifyReport {
    match outcome {
        InstallOutcome::Mcp { id } => {
            let Some(mcp) = mcp else {
                return VerifyReport { ok: false, detail: "MCP manager unavailable".into() };
            };
            // Implementer-verify the exact handle accessor: server info + health
            // + tool list. Expected shape: mcp.get_server_info(id) -> Option<McpServerInfo>
            // carrying HealthStatus + a tool count (or mcp.list_tools(id).len()).
            match mcp.get_server_info(id).await {
                Some(info) => {
                    let running = matches!(info.health, crate::mcp::manager::HealthStatus::Running);
                    verdict(running, info.tool_count())
                }
                None => VerifyReport { ok: false, detail: format!("server '{id}' not found") },
            }
        }
        InstallOutcome::Plugin { path } => {
            if std::path::Path::new(path).exists() {
                VerifyReport { ok: true, detail: format!("plugin present at {path}") }
            } else {
                VerifyReport { ok: false, detail: format!("plugin path missing: {path}") }
            }
        }
    }
}
```

> **Implementer-verify:** the precise `McpManagerHandle` accessor for server info, the `HealthStatus` variant name for "healthy/running", and how to obtain a tool count (a field, or `list_tools(id).len()`). Adjust the `verify_install` body to the real API; the pure `verdict()` (the tested core) does not change. If a single `get_server_info` accessor doesn't exist, compose from the available calls (e.g. `is_running(id)` + `list_tools(id)`).

- [ ] **Step 4: Run → PASS** `cargo test -p alephcore --lib store::verify::tests`. Add `pub mod verify;` to `src/store/mod.rs`.

- [ ] **Step 5: Build gate** `cargo build --bin aleph-server` — clean.

- [ ] **Step 6: Commit** `feat(store): post-install verification (verdict + verify_install)`

---

## Task 3: STORE_TOOLS set + protected `store` builtin agent

**Files:**
- Modify: `src/agents/tool_sets.rs` (add `STORE_TOOLS` + `resolve()` arm)
- Modify: `src/agents/registry.rs` (`builtin_agents()` 8th entry, `normalize_agent_alias()` arm, count test 7→8)

**Interfaces:**
- Consumes: `AgentDef::{new, with_description, with_when_to_use, with_allowed_tool_sets, with_max_iterations}` (`types.rs`), `AgentMode::SubAgent`, `tool_sets::resolve`.
- Produces: `pub const STORE_TOOLS: &[&str]` (the 5 NAMEs); a `store` `AgentDef` in `builtin_agents()`; `"store"` alias.

- [ ] **Step 1: Add the failing test** to `src/agents/tool_sets.rs` (or its test module)

```rust
#[test]
fn store_tools_resolve_to_five_names() {
    let set = resolve("STORE_TOOLS").expect("STORE_TOOLS must resolve");
    assert_eq!(set.len(), 5);
    assert!(set.contains(&"store_install_run"));
    assert!(set.contains(&"store_catalog_sync"));
}
```

And update the builtin-count test in `src/agents/registry.rs` (currently asserts 7 at ~line 367) to 8, plus a presence assertion:

```rust
#[test]
fn test_builtin_agents_count() {
    let agents = builtin_agents();
    assert_eq!(agents.len(), 8);
    let store = agents.iter().find(|a| a.id == "store").expect("store builtin present");
    assert_eq!(store.mode, AgentMode::SubAgent);
    assert_eq!(store.source, AgentSource::Builtin);
    assert!(store.allowed_tool_sets.iter().any(|s| s == "STORE_TOOLS"));
    assert!(!store.is_tool_allowed("file_write")); // not in STORE_TOOLS
    assert!(store.is_tool_allowed("store_catalog_sync"));
}
```

- [ ] **Step 2: Run → FAIL** `cargo test -p alephcore --lib agents::tool_sets` and `cargo test -p alephcore --lib agents::registry`

- [ ] **Step 3: Add `STORE_TOOLS`** to `src/agents/tool_sets.rs` (alongside `READ_ONLY`/`INVESTIGATION`/`ASYNC_SAFE`):

```rust
/// Private tool set for the built-in `store` agent. Not granted to chat agents
/// (the wildcard hole on `main`/`verify` is closed via denied_tools — see Task 4).
pub const STORE_TOOLS: &[&str] = &[
    "store_catalog_sync",
    "store_fetch_docs",
    "store_resolve_spec",
    "store_install_run",
    "store_install_verify",
];
```
And add the arm to `resolve()`:
```rust
"STORE_TOOLS" => Some(STORE_TOOLS),
```

- [ ] **Step 4: Register the `store` builtin** as the 8th entry in `builtin_agents()` (`registry.rs:219`), after `verify`:

```rust
AgentDef::new("store", AgentMode::SubAgent)
    .with_description(
        "Extensions Store curator and installer: syncs the catalog, assigns \
         functional categories, and drives trust-gated installs. Built-in and \
         non-deletable. Cannot bypass install trust rails (disclosure + ack \
         + SHA256 are system-enforced).",
    )
    .with_when_to_use(
        "When curating the extensions catalog or installing an extension on the \
         user's behalf through the store.",
    )
    .with_allowed_tool_sets(vec!["STORE_TOOLS".into()])
    .with_max_iterations(15),
```
(`source` defaults to `AgentSource::Builtin` — no setter. `with_allowed_tool_sets` clears the `["*"]` default so only STORE_TOOLS is granted.)

- [ ] **Step 5: Add the `"store"` alias** in `normalize_agent_alias()` (`registry.rs:190`) as a first-class exact-match arm so `"store"`/`"Store"` resolve:
```rust
"store" => Some("store"),
```
**Implementer-verify** the match shape (the fn lowercases input first per existing arms) and mirror it.

- [ ] **Step 6: Run → PASS** both test modules. **Build gate** `cargo build --bin aleph-server` — clean (STORE_TOOLS lists names not yet registered as tools; that is fine at compile time — the names are `&str` literals; the agent simply sees no store tools until T5–T8 land).

- [ ] **Step 7: Commit** `feat(agents): STORE_TOOLS set + protected built-in store agent`

---

## Task 4: Generalize the delete guard + close the wildcard hole

**Files:**
- Modify: `src/builtin_tools/agent_manage/delete.rs` (generalize guard; inject catalog registry)
- Modify: `src/agents/registry.rs` (`denied_tools` on `main` + `verify`)
- Modify: the `AgentDeleteTool` construction site (wherever `AgentDeleteTool::new` is called — find via grep; likely `builder/constructor/mod.rs` or an agent_manage registration)

**Interfaces:**
- Consumes: `crate::agents::AgentRegistry::get(&self, id) -> Option<AgentDef>` (`registry.rs:50`), `AgentDef.source`, `crate::agents::AgentSource::Builtin` (`types.rs:12`), `AgentDef::with_denied_tools`.
- Produces: a deletion guard that rejects any `source == Builtin`; `main`/`verify` that deny the 5 store tools.

- [ ] **Step 1: Write the failing tests.**

(a) In `src/agents/registry.rs` test module — the wildcard-hole closure:
```rust
#[test]
fn wildcard_agents_deny_store_tools() {
    let agents = builtin_agents();
    let main = agents.iter().find(|a| a.id == "main").unwrap();
    let verify = agents.iter().find(|a| a.id == "verify").unwrap();
    assert!(!main.is_tool_allowed("store_install_run"));
    assert!(!verify.is_tool_allowed("store_install_run"));
    assert!(main.is_tool_allowed("flow_run")); // unrelated tools still allowed
}
```

(b) In `src/builtin_tools/agent_manage/delete.rs` test module — the guard. Construct an `AgentDeleteTool` with a catalog `AgentRegistry::with_builtins()` and assert a builtin id is rejected and an unknown id is allowed past the guard. **Implementer-verify** the tool's constructor signature; the test shape:
```rust
#[tokio::test]
async fn rejects_builtin_agent_deletion() {
    let tool = make_test_delete_tool(); // helper wiring catalog=with_builtins()
    let err = tool.call(AgentDeleteArgs { agent_id: "explore".into() }).await.unwrap_err();
    assert!(err.to_string().to_lowercase().contains("built-in"));
}
```
(If a full `call()` test is impractical because of the runtime-registry dependency, factor the guard into a pure helper `fn is_protected(catalog: &AgentRegistry, id: &str) -> bool` and unit-test that instead — preferred, since it isolates the security logic from the runtime registry.)

- [ ] **Step 2: Run → FAIL** `cargo test -p alephcore --lib agents::registry` and `cargo test -p alephcore --lib builtin_tools::agent_manage::delete`

- [ ] **Step 3: Close the wildcard hole.** In `builtin_agents()`, extend `main` and `verify` with `denied_tools` covering the 5 store tools. For `main` (currently `.with_allowed_tools(vec!["*".into(), "flow_run".into()])`), append:
```rust
.with_denied_tools(vec![
    "store_catalog_sync".into(), "store_fetch_docs".into(), "store_resolve_spec".into(),
    "store_install_run".into(), "store_install_verify".into(),
])
```
For `verify` (currently has `denied_tools = [file_write, file_edit]`), add the same 5 names to its existing denied list. **Implementer-verify** no other builtin retains a bare `["*"]` allowlist without these denials (per findings, only `main` and `verify` have wildcards; `coder`/`researcher`/`plan`/`explore`/`default` use explicit allowlists or tool-sets and cannot reach store tools).

- [ ] **Step 4: Generalize the delete guard.** In `src/builtin_tools/agent_manage/delete.rs`:
  1. Add a field `agent_catalog: Arc<crate::agents::AgentRegistry>` to `AgentDeleteTool` and thread it through `new(...)`.
  2. Replace the `if args.agent_id == "main"` block (lines 91–96) with the generalized guard (a catalog miss → NOT builtin → allow delete):
```rust
// Reject deletion of any built-in agent (main + store + the other builtins).
if let Some(def) = self.agent_catalog.get(&args.agent_id) {
    if def.source == crate::agents::AgentSource::Builtin {
        return Err(crate::error::AlephError::other(format!(
            "Cannot delete the built-in '{}' agent. Built-in agents are protected.",
            args.agent_id
        )));
    }
}
```
  3. Update the construction site (grep `AgentDeleteTool::new`) to pass a shared `Arc<crate::agents::AgentRegistry>`. Reuse the existing catalog registry if one is already constructed for `AgentInfoTool` (findings: `info.rs` already injects `crate::agents::AgentRegistry`); otherwise build via `AgentRegistry::with_builtins()`.

- [ ] **Step 5: Run → PASS** both test modules. **Build gate** `cargo build --bin aleph-server` — clean.

- [ ] **Step 6: Commit** `feat(agents): protect all built-in agents from deletion + deny store tools to chat agents`

---

## Task 5: Store tools module + `store_catalog_sync` (wiring template)

**Files:**
- Create: `src/builtin_tools/store/mod.rs`, `src/builtin_tools/store/catalog_sync.rs`
- Modify: `src/builtin_tools/mod.rs` (`pub mod store;` + re-exports)
- Modify: the 5 wiring sites (see **Tool wiring checklist**)

**Interfaces:**
- Consumes: `crate::store::provider::{build_default_registry, ProviderRegistry::sync_all_into}` (`provider/mod.rs:86`, `registry_builder.rs:12`), `crate::store::cache::CatalogCache`, the marketplace configs used by the gateway handlers; `AlephTool` (`src/tools/traits.rs:64`).
- Produces: `StoreCatalogSyncTool { cache: Arc<CatalogCache>, marketplaces: HashMap<String, MarketplaceConfig> }`; `NAME = "store_catalog_sync"`; `Args` (empty / `{}`); `Output { synced: Vec<(String, usize)>, failed: Vec<(String, String)> }`.

- [ ] **Step 1: Write the failing test** (`catalog_sync.rs`, the pure result-shaping core):
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::provider::SyncReport;

    #[test]
    fn output_from_report() {
        let rep = SyncReport {
            synced: vec![("mcp-official".into(), 12)],
            failed: vec![("docker-mcp".into(), "timeout".into())],
        };
        let out = StoreCatalogSyncOutput::from_report(&rep);
        assert_eq!(out.synced, rep.synced);
        assert_eq!(out.failed.len(), 1);
    }
}
```

- [ ] **Step 2: Run → FAIL** `cargo test -p alephcore --lib builtin_tools::store::catalog_sync::tests`

- [ ] **Step 3: Implement** `catalog_sync.rs`:
```rust
//! `store_catalog_sync` — run all provider syncs into the local cache.
//! Categorization (Task 1) runs inside sync_all_into, so this also refreshes
//! functional categories. The deterministic curation entry point.
use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::store::cache::CatalogCache;
use crate::store::provider::{build_default_registry, SyncReport};
use crate::tools::AlephTool;
// MarketplaceConfig path: implementer-verify the exact import the gateway uses.
use crate::config::types::... ::MarketplaceConfig;

#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct StoreCatalogSyncArgs {}

#[derive(Debug, Clone, Serialize)]
pub struct StoreCatalogSyncOutput {
    pub synced: Vec<(String, usize)>,
    pub failed: Vec<(String, String)>,
}
impl StoreCatalogSyncOutput {
    #[must_use]
    pub fn from_report(r: &SyncReport) -> Self {
        Self { synced: r.synced.clone(), failed: r.failed.clone() }
    }
}

#[derive(Clone)]
pub struct StoreCatalogSyncTool {
    pub cache: Arc<CatalogCache>,
    pub marketplaces: HashMap<String, MarketplaceConfig>,
}

#[async_trait]
impl AlephTool for StoreCatalogSyncTool {
    const NAME: &'static str = "store_catalog_sync";
    const DESCRIPTION: &'static str =
        "Sync all extension sources into the local catalog cache and refresh functional categories.";
    type Args = StoreCatalogSyncArgs;
    type Output = StoreCatalogSyncOutput;

    async fn call(&self, _args: Self::Args) -> Result<Self::Output> {
        let registry = build_default_registry(self.marketplaces.clone());
        let report = registry.sync_all_into(&self.cache).await;
        Ok(StoreCatalogSyncOutput::from_report(&report))
    }
}
```
**Implementer-verify:** the `MarketplaceConfig` import path (the gateway start builder converts `plugin_marketplaces` → `MarketplaceConfig`; mirror that exact type) and that `SyncReport` fields are `pub` (findings: `synced`/`failed`).

- [ ] **Step 4: Create `src/builtin_tools/store/mod.rs`** (`pub mod catalog_sync; pub use catalog_sync::StoreCatalogSyncTool;`) and add `pub mod store;` to `src/builtin_tools/mod.rs`.

- [ ] **Step 5: Wire the tool** at all 5 sites per the **Tool wiring checklist** (definitions catalog, `with_config` construction with injected `cache` + `marketplaces`, struct field `store_catalog_sync: Option<StoreCatalogSyncTool>`, `execute_tool` arm, `register_core_tools` schema). **Resolve the integration seam here** (the T5 implementer-verify note): get the `CatalogCache` + marketplace configs into `with_config`.

- [ ] **Step 6: Run → PASS** the unit test. **Build gate** `cargo build --bin aleph-server` — clean, no warnings.

- [ ] **Step 7: Commit** `feat(store): store_catalog_sync builtin tool + store tools module wiring`

---

## Task 6: `store_resolve_spec` + `store_fetch_docs` (scaffold)

**Files:**
- Create: `src/builtin_tools/store/resolve_spec.rs`, `src/builtin_tools/store/fetch_docs.rs`
- Modify: `src/store/provider/mod.rs` (add `ProviderRegistry::resolve_for_entry`)
- Modify: `src/builtin_tools/store/mod.rs` + the 5 wiring sites (×2 tools)

**Interfaces:**
- Consumes: `ProviderRegistry::get` (`provider/mod.rs:71`), `SourceProvider::resolve_install_spec` (`provider/mod.rs:47`), `CatalogCache::query` + `CatalogFilter` (to load the entry by id), `InstallSpec`.
- Produces: `ProviderRegistry::resolve_for_entry(&self, entry) -> Result<InstallSpec, SourceError>`; `StoreResolveSpecTool` (`store_resolve_spec`, Args `{ entry_id: String }`, Output the resolved `InstallSpec` as JSON); `StoreFetchDocsTool` (`store_fetch_docs`, Args `{ url: String }`, Output `{ text: String, truncated: bool }`).

- [ ] **Step 1: Failing test** for the routing helper (`provider/mod.rs` test module):
```rust
#[tokio::test]
async fn resolve_for_entry_routes_by_source_id() {
    let reg = build_default_registry(Default::default());
    let mut e = sample_entry(); // helper: ExtensionEntry with source_id="local"
    e.source_id = "local".into();
    // 'local' has no provider → Err, not panic
    assert!(reg.resolve_for_entry(&e).await.is_err());
}
```

- [ ] **Step 2: Run → FAIL** `cargo test -p alephcore --lib store::provider`

- [ ] **Step 3: Implement `resolve_for_entry`** on `ProviderRegistry` (`provider/mod.rs`):
```rust
/// Route an entry to its provider and resolve its install spec.
pub async fn resolve_for_entry(
    &self,
    entry: &crate::store::types::ExtensionEntry,
) -> Result<crate::store::types::InstallSpec, SourceError> {
    let provider = self
        .get(&entry.source_id)
        .ok_or_else(|| SourceError::other(format!("no provider for source '{}'", entry.source_id)))?;
    provider.resolve_install_spec(entry).await
}
```
**Implementer-verify** the `SourceError` constructor (`::other` or equivalent) and the `get` borrow lifetime.

- [ ] **Step 4: Implement `store_resolve_spec`** (`resolve_spec.rs`): load the entry from `cache.query(CatalogFilter { id: Some(entry_id), ..Default::default() })`, take the first; call `registry.resolve_for_entry(&entry)`; serialize the `InstallSpec` to JSON as the output. Deps: `cache`, `marketplaces` (to build the registry). NAME `"store_resolve_spec"`.

- [ ] **Step 5: Implement `store_fetch_docs`** (`fetch_docs.rs`, **scaffold**): fetch `url` text via `reqwest` (already a dep), cap to a fixed byte budget (e.g. 64 KiB), return `{ text, truncated }`. **Run `crate::store::trust::scan_for_injection` on the fetched text** and include any findings in the output (curator-injection hardening, spec §11). Document in the module header that this tool is a scaffold for the deferred long-tail path and is not wired to any user install surface. NAME `"store_fetch_docs"`.

```rust
//! `store_fetch_docs` — fetch a repo/URL's README/manifest for the long-tail
//! install path. SCAFFOLD (v1): implemented + injection-scanned, but NOT wired
//! to any user-facing install flow. The supported install path is the
//! deterministic fast-path (P2/P3 UI).
```
**Implementer-verify** the shared `reqwest::Client` access (the start builder may already construct one; if not, build a short-timeout client locally).

- [ ] **Step 6: Add a small unit test** per tool (output shaping / truncation boundary). Run → PASS.

- [ ] **Step 7: Wire both tools** at the 5 sites. **Build gate** `cargo build --bin aleph-server` — clean.

- [ ] **Step 8: Commit** `feat(store): store_resolve_spec + store_fetch_docs (long-tail scaffold) + resolve_for_entry`

---

## Task 7: `store_install_run` (trust-gated) + SHA256 audit

**Files:**
- Create: `src/builtin_tools/store/install_run.rs`
- Modify: `src/store/install.rs` (thread `sha256` if the audit finds a gap)
- Modify: `src/builtin_tools/store/mod.rs` + 5 wiring sites

**Interfaces:**
- Consumes: `store::trust::build_disclosure` (`trust.rs:72`) + `DisclosurePayload.ack_required`, `store::install::run_install` (`install.rs:84`) + `InstallContext`/`InstallOutcome`, `store::secrets::{field_key, secret_ref}`, `ProviderRegistry::resolve_for_entry` (T6), `CatalogCache::query`, `SharedTokenManager::store_secret` (vault), `McpManagerHandle`, `MarketplaceManager`.
- Produces: `StoreInstallRunTool` (`store_install_run`); pure `fn gate(ack_required: bool, is_oci: bool) -> GateOutcome`.

**Args:** `{ entry_id: String, config_values: serde_json::Map<String, Value> }` — **deliberately NO `ack` field.** The store agent is an LLM; an `ack` argument it controls would let it fabricate user consent. **Output:** `enum InstallToolResult { NeedsUserConsent { disclosure }, Installed { outcome }, Rejected { reason } }`.

**Security design (gate-faithful — the binding rule, from Global Constraints "the agent cannot self-ack"):** this tool has NO ack input and never completes an ack-required install. Any spec whose disclosure sets `ack_required` (community / LLM-derived / stdio MCP — the risky classes) returns `NeedsUserConsent { disclosure }` with **zero install or secret-storage side effects**; the user must complete it through the trust-gated UI (`extensions.install`, which the P3 flow drives only after a real user gesture). The agent may auto-complete only clean, no-ack-required specs (e.g. Official-tier). OCI is always rejected. The `ack=true` install branch exists ONLY in the `extensions.install` RPC path, never reachable from this tool.

- [ ] **Step 1: Failing test** for the pure gate (`install_run.rs`):
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oci_is_rejected() {
        assert_eq!(gate(false, true), GateOutcome::Reject);
    }
    #[test]
    fn oci_rejected_even_when_ack_required() {
        assert_eq!(gate(true, true), GateOutcome::Reject);
    }
    #[test]
    fn ack_required_bounces_to_user() {
        assert_eq!(gate(true, false), GateOutcome::NeedsUserConsent);
    }
    #[test]
    fn clean_spec_proceeds() {
        assert_eq!(gate(false, false), GateOutcome::Proceed);
    }
}
```

- [ ] **Step 2: Run → FAIL** `cargo test -p alephcore --lib builtin_tools::store::install_run::tests`

- [ ] **Step 3: Implement the gate + tool.** Gate logic (the security core):
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateOutcome { Reject, NeedsUserConsent, Proceed }

/// System-enforced install gate. OCI is always rejected. Any ack-required spec
/// bounces to the user (`NeedsUserConsent`) — the agent has NO way to satisfy
/// the ack, so it cannot self-approve a risky install. Only clean (no-ack)
/// specs proceed to a direct agent-driven install.
#[must_use]
pub fn gate(ack_required: bool, is_oci: bool) -> GateOutcome {
    if is_oci { return GateOutcome::Reject; }
    if ack_required { return GateOutcome::NeedsUserConsent; }
    GateOutcome::Proceed
}
```
`call()` flow: load entry by id → `resolve_for_entry` → `build_disclosure(&entry, &spec)` → `gate(disclosure.ack_required, spec_is_oci)`:
  - `Reject` → `Rejected { reason }` (no side effect).
  - `NeedsUserConsent` → `NeedsUserConsent { disclosure }` (**no install, no secret storage** — the agent surfaces the disclosure and directs the user to install via the store UI's ack flow).
  - `Proceed` → for each secret field in the disclosure, `store_secret(field_key(...), value)` from `config_values`; build `InstallContext { secret_refs, plain_values, mcp, marketplace, entry }`; `run_install(&spec, &ctx)` → `Installed { outcome }`.

This reuses P2's `handle_install` primitives (`build_disclosure`/`run_install`/`field_key`/`secret_ref`/`store_secret`) but the agent path **cannot reach the `ack=true` install branch** — that branch is exclusive to `extensions.install` driven by a real user gesture. Secrets are stored ONLY on the `Proceed` path (clean specs), never on `NeedsUserConsent`.

- [ ] **Step 4: SHA256 audit (mandatory).** Read `MarketplaceManager::install_to_scope` (`src/extension/marketplace/installer.rs`). Determine whether it verifies the artifact SHA256 (via the marketplace manifest's pinned hash, independent of the spec's `sha256`).
  - **If it verifies internally** (manifest-pinned hash → `verify_plugin_integrity`): document that the spec's `InstallSpec::GitDir.sha256` is redundant for marketplace plugins; no code change. Record the finding.
  - **If it does NOT verify** (the `None` 4th arg means "skip"): thread the spec's `sha256` through `run_install` (`install.rs:114`, change `None` → `spec_sha256.as_deref()`) and confirm `install_to_scope` calls `verify_plugin_integrity(path, Some(sha))`. This is a **P2 security regression fixed here**; add a regression test in `store::install` asserting a mismatched hash is rejected. **Surface this outcome to the controller** (it determines whether T7 includes a security fix).

- [ ] **Step 5: Run → PASS** the gate tests (+ the SHA256 regression test if added). Wire the tool at the 5 sites (deps: cache, marketplaces, mcp handle, marketplace manager, vault token manager). **Build gate** clean.

- [ ] **Step 6: Commit** `feat(store): store_install_run trust-gated install tool` (+ `fix(store): enforce SHA256 on plugin install` if the audit found a gap)

---

## Task 8: `store_install_verify`

**Files:**
- Create: `src/builtin_tools/store/install_verify.rs`
- Modify: `src/builtin_tools/store/mod.rs` + 5 wiring sites

**Interfaces:**
- Consumes: `store::verify::{verify_install, VerifyReport}` (T2), `store::install::InstallOutcome`, `McpManagerHandle`.
- Produces: `StoreInstallVerifyTool` (`store_install_verify`, Args `{ outcome: InstallOutcome }` or `{ kind: String, id_or_path: String }`, Output `VerifyReport`).

- [ ] **Step 1: Failing test** (output passthrough shaping):
```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn maps_plugin_outcome_args() {
        let a = StoreInstallVerifyArgs { kind: "plugin".into(), id_or_path: "/tmp/x".into() };
        let outcome = a.to_outcome().unwrap();
        assert!(matches!(outcome, crate::store::install::InstallOutcome::Plugin { .. }));
    }
}
```

- [ ] **Step 2: Run → FAIL** `cargo test -p alephcore --lib builtin_tools::store::install_verify::tests`

- [ ] **Step 3: Implement.** Args carry enough to reconstruct an `InstallOutcome` (`kind` + `id_or_path`); `to_outcome()` builds it; `call()` invokes `verify_install(&outcome, self.mcp.as_deref())` and returns the `VerifyReport`. Dep: optional `McpManagerHandle`.

- [ ] **Step 4: Run → PASS.** Wire at the 5 sites. **Build gate** `cargo build --bin aleph-server` — clean.

- [ ] **Step 5: Commit** `feat(store): store_install_verify builtin tool`

---

## Self-Review (author checklist, run after writing — fix inline)

**1. Spec coverage (§9 Store Agent):**
- Registration in `builtin_agents()`, SubAgent, Builtin → **T3** ✅
- Protection / generalized delete guard → **T4** ✅ (panel-hide N/A — finding #3)
- `STORE_TOOLS` + 5 private tools, scoped, not exposed to chat → **T3** (set) + **T4** (deny hole) + **T5–T8** (tools) ✅
- Internal driving via spawn → store agent is registered + tool-scoped; on-demand spawn uses the existing `subagent_spawner` path (no new code needed for v1; no periodic scheduler — finding #4) ✅
- Curation: deterministic category map → **T1** ✅; LLM/featured/high-star deferred (findings #4, user decision) ✅
- Install model (§10): deterministic fast-path via `store_install_run` → **T7**; verify → **T2/T8**; long-tail scaffolded → **T6** ✅
- Trust rails (§11): disclosure + ack + SHA256 in `store_install_run` → **T7** (incl. SHA256 audit/fix) ✅; injection scan on fetched docs → **T6** ✅

**2. Placeholder scan:** Code blocks are complete for all tested cores (`categorize`, `verdict`, `gate`, `from_report`, `to_outcome`, `resolve_for_entry`). Wiring steps carry exact file:line anchors + explicit `Implementer-verify` notes for the three genuine seams (BuiltinToolConfig deps, McpManagerHandle accessor, MarketplaceConfig import). The one deliberate `...` placeholder is the `MarketplaceConfig` import path in T5 Step 3 — flagged as implementer-verify, not a silent gap.

**3. Type consistency:** The 5 tool NAMEs are identical across Global Constraints, `STORE_TOOLS` (T3), the deny list (T4), and each `AlephTool::NAME` (T5–T8). `InstallOutcome`/`InstallSpec`/`VerifyReport`/`SyncReport` names match the verified store-backend signatures. `AgentSource::Builtin`, `AgentMode::SubAgent`, `AgentDef` builder methods match `types.rs`.

**4. Task right-sizing:** 8 tasks, each an independently testable deliverable with its own TDD cycle and commit. T1 (categories) and T2 (verify) are pure + standalone. T3/T4 are registration/security (pure-ish tests). T5 establishes the tool-wiring template; T6–T8 repeat it. Dependency order: T1, T2 → T3 → T4 → T5 → {T6, T7, T8}; T8 depends on T2; T6/T7 depend on T5's module.

**5. Security focus:** T4 (delete guard + wildcard-hole) and T7 (install gate + SHA256) are the security-critical tasks — both have explicit adversarial tests (builtin-delete rejected, wildcard agents denied store tools, OCI rejected, ack-gate honored, SHA256 mismatch rejected). Dispatch their reviewers on the most capable model.
