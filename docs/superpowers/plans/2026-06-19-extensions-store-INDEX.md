# Unified Extensions Store — Plan Index (Master Roadmap)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement each phase plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Ship a unified, app-store-style "Extensions" surface in Aleph where MCP servers, plugins, and skills are one user-facing concept (`Extension`), browsed by **function** (not type), installed in one click, curated and installed by a built-in **Store Agent**, behind system-enforced trust rails.

**Architecture:** Thin federation. A new `src/store/` umbrella module owns store-facing types (`Extension*`) and a `SourceProvider` aggregation layer over existing backends; an `extensions.*` JSON-RPC façade delegates to the existing plugin/MCP/skill managers; a built-in non-deletable Store Agent drives discover→curate→install→verify; the Leptos panel renders a cached, functional-category catalog.

**Tech Stack:** Rust (tokio 1.35, rusqlite 0.37 bundled, reqwest 0.12, serde), Leptos/WASM + Tailwind panel, JSON-RPC over WebSocket.

**Spec:** `docs/superpowers/specs/2026-06-19-unified-extensions-store-design.md`
**UI mockup:** `docs/superpowers/specs/2026-06-19-extensions-store-mockup.html`

---

## Global Constraints

Every task implicitly includes these. Exact values, copied from the spec:

- **Umbrella naming:** new module `src/store/` owns `Extension`, `ExtensionEntry`, `ExtensionKind { Skill, Plugin, Mcp }`, `ExtensionCategory`, `InstallSpec`, `TrustTier`, `SourceProvider`. **Do NOT rename the existing `src/extension/` module** (it is the plugin backend). `ExtensionKind` is store-facing and distinct from the runtime `PluginKind { Wasm, Mcp, Static }`.
- **Thin façade:** `extensions.*` RPC delegates to existing handlers/managers; it does not reimplement business logic.
- **Reuse, do not fork:** marketplace install path (`verify_plugin_integrity` + `install_plugin_from_cache`/`update_plugin_from_cache`, atomic + SHA256), `config_schema` + `ConfigUiHint`, WASM credential injection, the agent subsystem (`builtin_agents()`, `subagent_spawner::spawn`, `AllowlistToolService`, `tool_sets`), JSON-RPC dispatcher (`HandlerRegistry::register`, `parse_params`).
- **Trust rails are system-enforced, never agent-discretionary:** pre-install disclosure (exact command + secrets + network/fs + version + SHA256 + tier), OS-keychain secrets, pin + re-gate-on-change. The Store Agent orchestrates but cannot bypass them.
- **Browse axis = functional `ExtensionCategory`** (primary); `kind` is a secondary badge/filter only.
- **No sandbox in v1** — informed-consent disclosure is the boundary; sandbox is a flagged fast-follow. Community/LLM-derived MCP installs require an explicit "I understand the risk" acknowledgement.
- **v1 sources only (mirror-safe):** Aleph/claude-plugins marketplaces (plugin), official MCP registry (mcp), Docker MCP catalog (mcp). ClawHub + GitHub crawler are deferred.
- **Catalog is cached locally** (rusqlite at `~/.aleph/store_catalog.db`); browse/filter is always served from cache; the network is touched only by background sync + `resolve_install_spec`.
- **Repo conventions:** `docs/` is gitignored — design/plan docs are **force-added** (`git add -f`). Work happens on branch `feat/unified-extensions-store`. Handlers return `JsonRpcResponse` directly; internal ops return `Result<T, String>`/typed errors converted at the boundary.

---

## Whole-feature file map

### New module: `src/store/`
| File | Responsibility | Phase |
|---|---|---|
| `src/store/mod.rs` | module exports | P0 |
| `src/store/types.rs` | `ExtensionKind`, `ExtensionCategory`, `TrustTier`, `McpTransport`, `EnvDecl`, `HeaderDecl`, `InstallSpec`, `ExtensionEntry` | P0 |
| `src/store/cache.rs` | rusqlite catalog cache (open/init/upsert/query) | P0 |
| `src/store/reconcile.rs` | map installed MCP/plugin/skill → `ExtensionEntry{installed:true}` | P0 |
| `src/store/provider/mod.rs` | `SourceProvider` trait, `ProviderRegistry`, `SyncCtx`, `Query`, `SourceError` | P1 |
| `src/store/provider/marketplace.rs` | plugin-marketplace provider (over `MarketplaceManager`) | P1 |
| `src/store/provider/mcp_registry.rs` | official MCP registry provider (reqwest) | P1 |
| `src/store/provider/docker_mcp.rs` | Docker MCP catalog provider (reqwest, YAML) | P1 |
| `src/store/install.rs` | kind-routed install over `InstallSpec` (deterministic path) | P2 |
| `src/store/trust.rs` | trust-tier assignment, disclosure payload, injection scan | P2 |
| `src/store/secrets.rs` | OS-keychain secret storage (reuse credential injection) | P2 |
| `src/store/agent.rs` | Store Agent wiring (curation job + install driver) | P4 |

### New gateway façade: `src/gateway/handlers/extensions/`
| File | Responsibility | Phase |
|---|---|---|
| `.../extensions/mod.rs` | params/response types + `register_extensions_handlers` | P0 |
| `.../extensions/catalog.rs` | `extensions.catalog`, `extensions.installed`, `extensions.detail` | P0/P1 |
| `.../extensions/lifecycle.rs` | `extensions.toggle`, `extensions.uninstall` | P0 |
| `.../extensions/install.rs` | `extensions.install`, `extensions.configure` (trust-gated) | P2 |
| `.../extensions/sources.rs` | `extensions.sources.{list,add,remove,refresh}` | P1 |

### New private agent tools: `src/builtin_tools/store/`
`store_catalog_sync.rs`, `store_fetch_docs.rs`, `store_resolve_spec.rs`, `store_install_run.rs`, `store_install_verify.rs` (P4).

### Frontend (Leptos): `interfaces/webchat/src/`
`views/extensions/` (browse/detail/installed), `components/json_schema_form.rs`, `components/extensions/*`, `api/extensions.rs`; modify `components/mode_sidebar.rs`, `components/nav_menu.rs`, `app.rs`, `locales/{en,zh}.json` (P3); demote `views/settings/{mcp,plugins,skills}.rs`, remove `clawhub` menu (P5).

### Modified (backend)
- `src/lib.rs` (or crate root) — add `pub mod store;`
- `src/gateway/handlers/mod.rs` — `pub mod extensions;`
- `src/bin/aleph-server/commands/start/builder/handlers/` — register `extensions.*` (capturing Mcp/Skill handles + extension manager)
- `src/agents/registry.rs`, `src/agents/tool_sets.rs`, `src/builtin_tools/agent_manage/delete.rs` (P4)
- `Cargo.toml` — add `serde_yaml` (Docker catalog) if not present (P1)

---

## Phase plans (dependency-ordered)

| Phase | Plan file | Goal | Depends on | Independently testable deliverable |
|---|---|---|---|---|
| **P0 Foundations** | `…-p0-foundations.md` | Store types + rusqlite cache + installed reconciliation + `extensions.*` façade (catalog from cache, installed, toggle, uninstall) | — | `extensions.installed` returns the user's real installed MCP/plugins/skills as unified `ExtensionEntry`s; toggle/uninstall delegate correctly |
| **P1 Source layer** | `…-p1-source-layer.md` | `SourceProvider` + `ProviderRegistry` + 3 providers + background sync into cache | P0 | `extensions.catalog` returns real entries fetched from the MCP registry / Docker / marketplaces, cached and browsable offline |
| **P2 Trust rails + install** | `…-p2-trust-install.md` ✅ detailed | Trust disclosure payload, injection scan, vault secrets (no OS keychain — corrected), per-server MCP secret injection at spawn, deterministic install routing, trust-gated `extensions.install`/`configure`, post-install verify | P0,P1 | `extensions.install` installs a clean-spec extension after a disclosure+ack gate; secrets land in the encrypted vault and inject per-server; SHA256 verified; OCI deferred |
| **P3 Store UI** | `…-p3-store-ui.md` (later) | Top-level Extensions mode, functional-category browse, detail drawer, config wizard (`json_schema_form`), installed view, install-guard | P0,P1,P2 | User browses by category, installs with wizard, returns to chat; mockup realized in Leptos |
| **P4 Store Agent** | `…-p4-store-agent.md` (later) | Built-in non-deletable `store` agent, private tools, background curation (categories/blurbs/featured), install ownership + verify, long-tail URL install | P0–P3 | Store Agent curates the catalog and drives installs end-to-end; cannot be deleted |
| **P5 Migration & i18n** | `…-p5-migration-i18n.md` (later) | Demote MCP/Plugins/Skills settings panels to "Advanced", remove ClawHub menu, en/zh strings | P3 | Old panels relabeled Advanced; ClawHub menu gone; store fully localized |

**Authoring policy:** P0, P1, and P2 are written in full bite-sized detail (P2 re-grounded against real P0/P1 interfaces + interface research that corrected the spec: no OS keychain → encrypted vault, OCI install deferred, per-server MCP secret injection). P3–P5 are detailed when their predecessors land. Each phase plan ends with working, independently testable software.

---

## Dependency graph

```
P0 ──► P1 ──► P2 ──► P3 ──► P5
                └────► P4 ─┘
(P4 depends on P0–P3; P5 depends on P3)
```

## Test strategy per phase
- **P0/P1/P2 (Rust core):** `cargo test -p alephcore <module>` unit tests; rusqlite uses in-memory DBs (`Connection::open_in_memory()`); providers tested against captured JSON fixtures (no network in unit tests). Per the memory note, scope test builds narrowly (single module) to avoid rustc OOM.
- **P3/P5 (Leptos):** component logic unit tests + manual run via the `run` skill; visual parity against the mockup.
- **P4 (agent):** the Store Agent's private tools are unit-tested in isolation; the curation/install loop is integration-tested with a fixture catalog.
