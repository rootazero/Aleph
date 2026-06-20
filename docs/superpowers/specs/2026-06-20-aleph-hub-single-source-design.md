# Aleph Hub — Single-Source Teardown Design

**Date:** 2026-06-20
**Status:** Approved (design); pending implementation plan
**Supersedes:** `2026-06-20-extension-hub-federation-design.md` (the federation/multi-source/local-dedup design — this spec **reverses** it)
**Scope boundary:** Aleph-side only. The Aleph-Hub website (curation/crawl/listing → publish to `hub.heyaleph.com`) is a separate project at `D:\Workspace\Aleph-Hub` and is **out of scope** here. The catalog artifact contract is the seam.

---

## 1. Problem & Reframe

The previous iteration built a **local federation**: multiple `SourceProvider`s as peers, local cross-source dedup, local keyword categorization, and local `source_id → label` provenance mapping. That keeps discovery logic inside every Aleph instance, which (a) re-introduces per-user variance in what gets surfaced and (b) violates the project's thin-harness / LLM-sovereignty redlines by accreting deterministic curation code in the core.

**The fix:** centralize all curation on the **Aleph-Hub website**. Aleph local collapses into a **pure single-source consumer** of one published catalog artifact. It does not *discover* — it *renders + installs*. Every user sees the identical, website-curated catalog ⇒ unified experience (统一体验).

This is R3 (core minimalism) + R7 (LLM sovereignty) + R10 (thin harness / "zero-consumer abstraction → delete") applied to the extension subsystem.

---

## 2. Locked Decisions

- **D1. Single source.** Browse surface = the one published Aleph Hub catalog (`https://hub.heyaleph.com/catalog.json`). The three live registry browse providers (MCP Registry / Docker MCP / Plugin Marketplace) are **removed as browse sources**. Their **install backends** (`McpManagerHandle`, `MarketplaceManager`) stay — catalog entries still route to them at install time.
- **D2. Collapse the provider abstraction.** Delete `SourceProvider` trait + `ProviderRegistry` + `build_default_registry`. `StaticHubProvider` is **promoted** to a standalone `AlephHubCatalog` client (no trait, no registry).
- **D3. Provenance via catalog data.** Provenance is no longer computed locally from `source_id`. Each catalog entry carries an explicit upstream-origin string. Delete `display::source_label`; carry the label as a new `via` field on the entry, filled by the website.
- **D4. Categories prefilled.** The website assigns categories. Delete local `categorize` (keyword inference + the post-sync `Other` enrichment pass). The `ExtensionCategory` **enum stays** (core data model + wire format + filter param).
- **D5. No cross-source dedup locally.** Single source ⇒ delete `dedup`. (The website dedups across upstreams before publishing.)
- **D6. Install spec persisted in cache.** Add `install_spec` to the cached entry so resolution is a stateless cache lookup. Removes the in-memory spec map and the resolve-miss trap (see §6).
- **D7. Install paths.** Catalog-driven install only, retaining the full trust rails (disclosure → ack → `{{secret:NAME}}` vault → injection scan → `install_verify` LLM security review → `install_run`). The arbitrary-URL "LLM escape hatch" is **net-new and deferred** to a separate fast-follow spec (§9). Long-tail repos not yet curated remain installable via the main-loop LLM's general tools (without hub rails) until then.
- **D8. Rename.** Sidebar + view title → **"Aleph Hub"** (i18n value change only). Keep i18n keys, the `extensions.*` RPC namespace, and the `src/hub` module name unchanged. Chinese display name uses the English brand "Aleph Hub" for now (TBD).
- **D9. Remove `extensions.sources.*` RPC.** `sources.list` / `sources.refresh` have **zero UI consumers**, and on-demand refresh is already covered by the `hub_catalog_sync` tool. Per R10 (zero-consumer → delete), remove the handlers + their registration + the unused panel API stubs.
- **D10. Single endpoint const.** `ALEPH_HUB_URL = "https://hub.heyaleph.com/catalog.json"` (replaces the stale `hub.aleph.computer` placeholder), one named const in the new client.
- **D11. Future kinds, same catalog.** Themes / Workflows / Mini-apps are future `ExtensionKind` values inside the **same** artifact (schema-driven, additive). Not implemented now; the contract must remain forward-compatible (serde `default` on new fields).

---

## 3. Architecture: Before → After

```
BEFORE (federation)                         AFTER (single-source consumer)
─────────────────────                       ──────────────────────────────
ProviderRegistry                            AlephHubCatalog (one client)
 ├─ McpRegistryProvider  (browse)             ├─ fetch() HTTP → artifact
 ├─ DockerMcpProvider    (browse)             ├─ schema_version check
 ├─ MarketplaceProvider  (browse)             ├─ scan_for_injection (defense-in-depth)
 └─ StaticHubProvider    (browse)             ├─ into_entry() normalize (+via, +install_spec)
sync_all_into → categorize → cache            └─ sync_into(cache)  [single source]
read path: cache → dedup → source_label     read path: cache → (entry already carries via + spec)
install: registry.resolve_for_entry         install: cache lookup → entry.install_spec
```

The install backends (`McpManagerHandle`, `MarketplaceManager`) and the trust/secret/verify pipeline are **unchanged** — they sit below `InstallSpec` and don't know about sources.

---

## 4. Component Change Map (grounded by consumer audit)

### 4.1 Delete entirely
| File | Reason |
|---|---|
| `src/hub/dedup.rs` | D5 — single source, no cross-source dups |
| `src/hub/display.rs` | D3 — provenance now per-entry `via` |
| `src/hub/categorize.rs` | D4 — categories prefilled by website |
| `src/hub/provider/mcp_registry.rs` | D1 — browse provider removed |
| `src/hub/provider/docker_mcp.rs` | D1 — browse provider removed |
| `src/hub/provider/marketplace.rs` | D1 — browse provider removed (`MarketplaceManager` backend survives, wired independently) |
| `src/hub/provider/registry_builder.rs` | D2 — no registry to build |
| `src/hub/provider/mod.rs` | D2 — trait + registry gone (relocate `SourceError`/report types, see §5) |

### 4.2 Modify
| File | Change |
|---|---|
| `src/hub/provider/static_hub.rs` → relocate to `src/hub/catalog_client.rs` | Promote to standalone `AlephHubCatalog`. Drop `impl SourceProvider`. Inherent methods: `new(...)`, `fetch() -> Result<Vec<ExtensionEntry>, CatalogError>`, `sync_into(&self, cache) -> SyncReport`, `resolve(&self, entry) -> ...` becomes a cache lookup (no in-memory map — see D6/§6). Keep HTTP fetch, schema-version check, `scan_for_injection`, `into_entry` normalization. |
| `src/hub/mod.rs` | Remove `pub mod categorize/dedup/display/provider;`. Add `pub mod catalog_client;`. Keep `cache/hub_catalog/install/reconcile/secrets/trust/types/verify`. Keep the `ExtensionCategory` re-export. Update the module doc comment to point at **this** spec (was federation spec). |
| `src/hub/types.rs` | Add `pub via: Option<String>` and `pub install_spec: Option<InstallSpec>` to `ExtensionEntry`, both `#[serde(default, skip_serializing_if = "Option::is_none")]`. Update in-file test helper. `ExtensionCategory` unchanged. |
| `src/hub/hub_catalog.rs` | Add `via: Option<String>` to `HubCatalogEntry` (serde default). `into_entry()`: populate `via` (fallback to `manifest.name` so a label is always present) and `install_spec` (from the entry's spec). Keep `SUPPORTED_SCHEMA_VERSION`, manifest, tests. |
| `src/hub/cache.rs` | No schema/API change — `install_spec` + `via` persist for free inside the entry's serialized `data` JSON. Verify round-trip in a test. |
| `src/hub/reconcile.rs` | `base_entry()` struct literal: add `via: None, install_spec: None`. (Installed-item entries have no upstream provenance/spec.) No logic change. |
| `src/gateway/handlers/extensions/catalog.rs` | Remove `dedup` + `display` imports. Delete the dedup block. Emit the `source_label` **wire key** from `e.via.clone().unwrap_or_default()` (panel contract unchanged). Keep `CatalogParams` + `ExtensionCategory`. `handle_installed` untouched. |
| `src/gateway/handlers/extensions/install.rs` | Swap `Arc<ProviderRegistry>` → `Arc<AlephHubCatalog>` (or drop the param and read spec from cache). `resolve_spec()` helper: read `entry.install_spec` from the cached entry instead of registry routing. Keep `build_disclosure`, `scan_for_injection`, `split_fields`, `missing_required`, vault/secret logic, OCI rejection, `run_install`, `verify_install`. |
| `src/gateway/handlers/extensions/sources.rs` | **Delete** (D9). |
| `src/gateway/handlers/extensions/mod.rs` | Drop the `sources` re-export. |
| `src/builtin_tools/hub/catalog_sync.rs` | Drop `build_default_registry` + `SyncReport` imports. Build `AlephHubCatalog::new(...)`, call `sync_into(&cache)`. Output uses the client's small `SyncReport`. Update test. |
| `src/builtin_tools/hub/resolve_spec.rs` | Drop `build_default_registry`. After cache lookup, return `entry.install_spec` directly (cache lookup, no registry). Update tests (drop the "unknown provider" case → assert on missing cached spec; add `via/install_spec` to the sample entry). |
| `src/builtin_tools/hub/install_run.rs` | Drop `build_default_registry`. Resolve spec from the cached entry (`entry.install_spec`). **Keep** the `marketplaces` field + the `MarketplaceManager` built for `InstallContext` (GitDir plugin installs need it). Keep gate/consent/disclosure/run_install. |
| `src/builtin_tools/hub/install_verify.rs` | Untouched (zero registry/dedup/categorize/display deps). |
| `src/builtin_tools/hub/fetch_docs.rs` | Untouched (pure HTTP + `scan_for_injection`). |
| `src/bin/aleph-server/commands/start/mod.rs` | Replace `build_default_registry(marketplace_configs)` with `Arc<AlephHubCatalog>::new(ALEPH_HUB_URL, ...)`. 6h loop → `catalog.sync_into(&cache)`. Keep the independent `MarketplaceManager` (install backend) + `marketplace_configs` for it. Remove `register_extensions_sources_handlers` call (D9). |
| `src/bin/aleph-server/commands/start/builder/handlers/extensions.rs` | Swap `ProviderRegistry` → `AlephHubCatalog` in `register_extensions_install_handlers`. Remove `register_extensions_sources_handlers` (D9). `register_extensions_handlers` (catalog/installed/toggle/uninstall) untouched. |
| `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs` | Keep `catalog_cache` + `hub_marketplace_configs` ctor params (still needed by install backend). Adjust tool construction if a tool drops its `marketplaces` field. |
| `interfaces/webchat/src/api/extensions.rs` | Remove the unused `sources_list` / `sources_refresh` method stubs + `SourceInfo` (D9). `ExtensionEntry` mirror keeps `source_label` (wire key unchanged). |
| `interfaces/webchat/locales/{en,zh}.json` | `nav.extensions` + `extensions.title` → "Aleph Hub". Refresh `extensions.subtitle` (currently "Curated by your Store Agent" / "由你的商店智能体策展" — Store Agent retired) to Aleph-Hub wording. Keep `extensions.via`. |

### 4.3 Survives untouched (confirmed)
`src/hub/{install,verify,trust,secrets,cache}.rs` · `src/builtin_tools/hub/{install_verify,fetch_docs}.rs` · `src/extension/marketplace/*` (install backend) · `src/gateway/handlers/extensions/lifecycle.rs` (toggle/uninstall). The LLM-provider registries (`src/providers/registry.rs`, `src/generation/registry.rs`, `src/thinker/mod.rs`, `auth_profile_registry.rs`) are **grep false positives** — different "registry", do not touch.

---

## 5. Relocating the deleted shared types

`provider/mod.rs` (deleted) defines `SourceError`, `SyncReport`, `SyncCtx`, `Query`. After deletion:
- `SyncCtx`, `Query` — drop (only the trait used them).
- `SourceError` — replace with a small local `CatalogError` enum on `AlephHubCatalog` (`Network`/`Parse`/`Schema`/`Other`) or `anyhow`. The new client + the install/catalog handler error mapping reference it.
- `SyncReport` — define a minimal `{ synced: usize, failed: Vec<String> }` on the client; `catalog_sync`'s output type + test depend on its shape.

---

## 6. Install-spec persistence (the resolve-miss fix)

**Problem found:** the old code rebuilt the registry per tool call; `StaticHubProvider` cached `InstallSpec` in an in-memory `Mutex<HashMap>` populated only by `sync()`. A single long-lived `AlephHubCatalog` (or a fresh per-call instance) would have an empty spec map ⇒ `resolve` misses.

**Fix (D6):** persist the spec with the entry. `HubCatalogEntry` already carries the spec; `into_entry` writes it to `ExtensionEntry.install_spec`; the cache stores the full entry as JSON ⇒ spec persists across instances and restarts. Resolution becomes: `cache.query(by id) → entry.install_spec`. No in-memory map, no re-fetch, no shared-state wiring. This also lets the single 6h sync remain the only writer while every reader (handlers + tools) resolves from cache.

`install_spec` is non-sensitive public metadata (the same command the disclosure already shows the user), so flowing it to the panel via `extensions.catalog` is acceptable; `skip_serializing_if = None` keeps payloads lean for installed/local entries that have no spec.

---

## 7. Catalog contract (seam with the Aleph-Hub website)

The website publishes `https://hub.heyaleph.com/catalog.json` conforming to `HubCatalogArtifact` (manifest + entries), `schema_version = 1`. Per-entry fields the website **must** provide:
- `repo_url` — **mandatory** (open-source attribution; upstream link).
- `via` — upstream origin label (e.g. `"clawhub"`, `"hermes-atlas"`, `"github:owner"`, developer name). New, serde-default → additive/back-compat.
- `category` — prefilled (no local inference). Use `ExtensionCategory::Other` only when genuinely uncategorized.
- `install_spec` — the resolved spec (MCP stdio/remote, GitDir plugin; OCI is rejected at install).

Adding `via` is additive: old artifacts without it still parse (`default = None`, falls back to `manifest.name`).

---

## 8. UI / rename

- `nav.extensions`, `extensions.title` → "Aleph Hub" (en + zh). `extensions.subtitle` refreshed.
- Keep: card/detail browse, category chips (driven by entry `category`), kind/trust filters, search, install flow (trust modal → configure → verify → done), installed panel, the `via {label}` badge + `repo_url` link (already wired; backend now feeds `via`).
- No source-management UI exists to delete.

---

## 9. Out of Scope (explicit)

- **Arbitrary-URL LLM escape hatch** (D7) — net-new; separate fast-follow spec that designs the trust rails (disclosure/ack/vault/injection) into a URL/inline-spec install path. Until then, long-tail installs go through the main-loop LLM's general tools.
- **Aleph-Hub website** (curation, crawl, listing/上架, publish pipeline) — separate repo `D:\Workspace\Aleph-Hub`.
- **ClawHub / Hermes Atlas adapters** — they are *upstreams the website aggregates from*, not local sources.
- **Artifact signing**, multi-hub opt-in, themes/workflows/mini-apps kinds — future.

---

## 10. Risks & Migration

- **Struct-literal breakage:** adding two `ExtensionEntry` fields breaks every struct literal that lacks them. Enumerate + fix all sites (reconcile.rs `base_entry`; test helpers in cache.rs, types.rs, trust.rs, the hub tool tests, the new client tests). Serde `default` covers deserialization of old cache rows.
- **Cache back-compat:** existing `hub_catalog.db` rows deserialize fine (`via`/`install_spec` default to `None`); they gain values on the next sync. No migration needed.
- **RPC removal (D9):** confirm no non-panel client calls `extensions.sources.*` (audit found none). If a future client needs "refresh," the `hub_catalog_sync` tool covers it.
- **Doc drift:** mark the federation spec superseded; fix `hub/mod.rs` doc comment.
- **Stale URL:** ensure the new `heyaleph.com` const is the only hub URL; remove `hub.aleph.computer`.
- **Memory:** update the `extensions-store-progress` long-term memory after implementation (federation → single-source).

---

## 11. Testing & Verification

Per project convention (build is memory-heavy; minimize cargo): unit tests live in-file (`into_entry` sets `via`/`install_spec`; cache round-trips them; `catalog_sync` output shape; `resolve` reads spec from cache; catalog handler emits `source_label` from `via`). Verification scoped to `cargo check -p alephcore --lib` (lib) and `--bin aleph-server` (lib+bin) once at the end of a phase, not per edit; panel via `cargo check -p aleph-panel --target wasm32-unknown-unknown`. Full `cargo test` stays blocked by the pre-existing broken `tests/cancellation_chain.rs` → scope to `--lib`. No `just wasm` runtime rebuild unless explicitly requested.

---

## 12. Net effect

Deletes ~8 files (provider trait+registry+3 browse providers + dedup + display + categorize, ≈600+ LOC of multi-source scaffolding), replaces them with one ~200-LOC `AlephHubCatalog` client, and turns install-spec resolution into a stateless cache read. The hub becomes a thin single-source consumer — discovery and curation move entirely to the website.
