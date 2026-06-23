# MCP → Aleph Hub Convergence — Design Spec

**Date:** 2026-06-23
**Status:** Approved (design); pending spec review → implementation plan
**Scope:** Aleph main repo only. MCP convergence. Skills/Plugins convergence are explicit follow-ons (§10).

**Relationship to prior specs:**
- **Honors** `2026-06-20-aleph-hub-single-source-design.md` (D1 single remote source, D5 no local dedup; R3/R7/R10). This spec adds **no peer source and no local dedup** — only a cold-start *cache primer* into the existing `aleph-hub` source slot (§4).
- **Complements** `2026-06-21-mcp-store-unification-design.md` (which unified the Settings *manual CRUD* store onto the actor store). This spec unifies the *preset discovery/install* surface. Different seam, same end-state: one MCP store, one Hub.
- **Consumes** `2026-06-23-siliconflow-mcp-design.md`: `catalog.json` is the official-MCP authoring source. SiliconFlow (the 5th preset) flows through this pipeline automatically once added there.

---

## 1. Problem

Official MCPs reach the user through **two parallel discovery/install surfaces** with **incompatible server-id derivations**:

| | Built-in preset surface (the rogue path) | Extensions Hub |
|---|---|---|
| Catalog | `src/mcp/presets/catalog.json` (embedded) | `https://hub.heyaleph.com/catalog.json` → `CatalogCache` |
| RPCs | `mcp.list_presets` / `mcp.install_preset` | `extensions.catalog/disclosure/configure/install/toggle/uninstall` |
| UI | Settings ▸ MCP "Recommended" (shipped in `ea307c45c`) | `views/extensions/` (browse + installed) |
| Server id on install | **`volcengine-veimagex`** (raw preset slug) | **`aleph_hub_volcengine_veimagex`** (`mcp_server_id(entry.id)`) |

Installed-status in the Hub is reconciled by deriving `local:mcp:{mcp_server_id(entry.id)}` (`gateway/handlers/extensions/catalog.rs:71`) and matching the live server set. So a preset installed via **Settings ▸ MCP** persists `volcengine-veimagex`, which the Hub's reverse-lookup never matches → **the Hub shows it as not installed**. That is the concrete bug behind "安装后能在 Aleph hub 体现已安装".

Root cause: the built-in preset surface is a **second discovery/install path** that bypasses the single-source Hub. The fix is to **retire it** and make the Hub the one MCP surface, while ensuring official MCPs are present in the Hub catalog even before the website catalog is first fetched.

---

## 2. Goal

1. **One** MCP discovery + install + installed-status surface: the Aleph Hub.
2. Official MCPs (`catalog.json`) appear in the Hub **offline / before first remote fetch / independent of website deploy cadence** — they ship with the signed binary.
3. Installing an official MCP (from anywhere) reflects as **installed** in the Hub.
4. No reversal of the single-source architecture: no peer source, no local dedup, no local curation logic.

### 2.1 Convergence principle (all kinds) & the clearinghouse model

**Aleph Hub is the authority for all extensions — skills, plugins, and MCP.** This spec implements it for MCP; Skills and Plugins are follow-ons (§10) that reuse the same shape (cold-start primer + Hub-driven install + reconcile + the §9 migration). The Hub is a **clearinghouse (集散地), not a source host**: a catalog entry carries a *pointer* to the upstream source and install fetches the actual code from there —
- skills/plugins → `GitDir` (git clone from the GitHub repo),
- stdio MCP → `uvx --from git+https://…` / `npx <pkg>` (fetched at runtime from GitHub / npm),
- remote MCP → the published endpoint URL.

The Hub never stores extension source; it curates *what exists* and *where to get it*.

---

## 3. Locked Decisions

- **D1. Retire the rogue path.** Remove `mcp.list_presets` + `mcp.install_preset` handlers + their registration. All MCP discovery/install goes through `extensions.*`.
- **D2. Cold-start primer, same source slot (Path C).** At boot, **iff** the cache holds zero `aleph-hub` rows, project `presets::catalog()` → `ExtensionEntry`s and `cache.replace_source("aleph-hub", primer_entries)`. The async remote fetch later calls `replace_source("aleph-hub", remote_entries)` and **overwrites** the primer wholesale. Steady-state = pure remote; **zero local dedup**, **no peer source**.
- **D3. Remote id scheme is canonical.** Primer entry id = `aleph-hub:<slug>`, `slug` = the `catalog.json` preset id. This reuses the existing `mcp_server_id(entry.id)` install→reconcile chain unchanged. (Raw-slug ids are **not** used.)
- **D4. Install via the Hub engine.** Official MCP installs through `extensions.install` → existing `run_install` (`src/hub/install.rs`). Secrets route through the **vault** (`{{secret:NAME}}`) exactly like every other Hub install — official MCP keys are no longer plaintext.
- **D5. Trust = website-authoritative once reachable.** The primer stamps `trust_tier: Official` (it is the signed binary). After a successful fetch, the website's published tiers apply (the website is Aleph's own, trusted — consistent with single-source). **No remote-trust clamp** (that belonged to the rejected peer-seed model).
- **D6. Settings ▸ MCP "Recommended" is re-pointed, not a second path.** The cards/dialog from `ea307c45c` are kept but re-pointed to read `extensions.catalog` (kind=mcp, installed=false) and install via `extensions.install`. It becomes a thin *view* of the one Hub catalog — same engine, same ids. (Alternative: remove it entirely and centralize discovery in the Hub view — see §11 Q1.)
- **D7. Preset *engine* retires; preset *data* stays.** `InstallPlan` / `plan_install` / `missing_required_env` retire (zero consumers after D1). `catalog.json` + `presets::catalog()` + the `McpPreset`/`PresetTransport`/`PresetEnvVar` types **stay** (the primer reads them). `is_runtime_available` is **kept/extracted** for the NoRuntime check (D8).
- **D8. Preserve the NoRuntime UX (small).** Add a runtime-availability check to the Hub pre-install path (`extensions.disclosure` or `run_install`) so a stdio MCP whose runtime (node/python) is absent reports NoRuntime instead of silently persisting a server that won't start. Reuses `is_runtime_available`.
- **D9. Migrate pre-Hub installs by delete + re-fetch.** Extensions installed before the Hub became authoritative can be orphaned (wrong id / untracked shape) and no longer reconcile. Per the user directive, on boot **remove** the orphaned install (local delete) so it is re-installed from the Hub. MCP is the **conservative, closed-set** case: a persisted server whose id exactly equals a *retired-preset slug* (`context7`/`amap`/`minimax`/`volcengine-veimagex`/`siliconflow`), **whose transport/command matches that preset**, and for which a cache entry `aleph-hub:<slug>` exists → `mcp.remove_server(id)` + one-time notice. The shape match keeps a user-custom server that happens to share the name untouched. **Consequence:** keys for those few servers are re-entered on re-install — a clean re-install avoids vault re-keying (vault entries are keyed by the old id); see §11 Q4.
- **D10. Hub = clearinghouse.** Catalog entries point to upstream source (GitHub / npm / remote endpoint); install fetches from there. The Hub hosts no source. (Already the `install_spec` model; stated for emphasis per the all-kinds directive.)

---

## 4. Architecture

```
BOOT (cache has 0 aleph-hub rows)
  presets::catalog()  ──project──▶  cache.replace_source("aleph-hub", primer_entries)
  (signed binary)                     trust_tier=Official, id=aleph-hub:<slug>, install_spec from transports

ASYNC remote fetch succeeds
  hub.heyaleph.com/catalog.json ──▶ cache.replace_source("aleph-hub", remote_entries)   # overwrites primer
  fetch fails → primer (or last-good) stays                                              # cold-start guarantee

QUERY
  extensions.catalog ──▶ read cache (one source: aleph-hub) ──▶ mark_installed (unchanged)

INSTALL
  extensions.install ──▶ run_install (McpStdio/McpRemote) ──▶ mcp.add_server
                          + vault {{secret:}}  + NoRuntime pre-check (D8)
  persisted id = mcp_server_id("aleph-hub:<slug>") = aleph_hub_<slug>
  reconcile: local:mcp:aleph_hub_<slug>  →  MATCH ✓
```

The primer is a **cache primer**, not a `SourceProvider`. There is still exactly one source slot (`aleph-hub`) and no dedup — fully consistent with the single-source design.

---

## 5. Cross-repo id contract (the one seam to get right)

For installed-status to survive the **primer → remote** transition, the primer and the published website catalog **must use the same entry id** for the same official MCP:

```
official MCP entry id  ==  "aleph-hub:" + <catalog.json preset id>
   e.g.  aleph-hub:volcengine-veimagex,  aleph-hub:siliconflow,  aleph-hub:amap, ...
```

Both sides derive this from the **same** source: Aleph core reads `catalog.json` (primer); the Aleph-Hub website pipeline ingests `catalog.json` (per the SiliconFlow spec) and emits `aleph-hub:<preset id>`. **Action:** verify the website's current official-MCP entry ids match this convention; align whichever side differs. (The user's recent Hub-side mirroring used `data/seeds/mcp-presets.json` — confirm its emitted ids.)

---

## 6. Components to change

| File | Change |
|---|---|
| `src/hub/` (new `official_seed.rs`, ~70 lines) | `fn primer_entries() -> Vec<ExtensionEntry>`: map each `McpPreset` → `ExtensionEntry { id: format!("aleph-hub:{}", p.id), kind: Mcp, source_id: "aleph-hub", trust_tier: Official, requires_config: !p.required_env.is_empty(), install_spec: Some(<from preferred transport>), category: <PresetCategory→ExtensionCategory>, installed:false, enabled:false }`. Transport→`InstallSpec` (stdio→McpStdio{command,args,env:EnvDecl[]}; remote→McpRemote). `required_env`→`EnvDecl`. |
| `src/bin/aleph-server/commands/start/...` (boot) | After `CatalogCache` open, **before** kicking the async hub sync: `if cache.count_source("aleph-hub") == 0 { cache.replace_source("aleph-hub", primer_entries()) }`. Warn-only on failure (never abort boot). |
| `src/bin/aleph-server/commands/start/...` (boot, after primer) | D9 migration: for each persisted MCP server whose id ∈ {retired-preset slugs}, whose transport/command matches that preset, and where the cache holds `aleph-hub:<slug>` → `mcp.remove_server(id)` + one-time notice. Idempotent (next boot: no orphan left). Warn-only; never abort boot. |
| `src/hub/cache.rs` | Add `count_source(source_id) -> usize` (or reuse an existing count) for the primer gate. |
| `src/hub/install.rs` (`run_install`) | Add D8 NoRuntime pre-check for stdio specs via `is_runtime_available`. (Secret/vault path already exists — no change.) |
| `src/mcp/presets/mod.rs` | Retire `InstallPlan` / `plan_install` / `missing_required_env`. Keep `catalog()` / `find()` / the data types. Keep/extract `is_runtime_available`. Keep the `bundled_catalog_parses…` test. |
| `src/gateway/handlers/mcp.rs` | Remove `handle_list_presets` / `handle_install_preset` + `preset_view` + params. |
| `src/bin/aleph-server/commands/start/builder/handlers/mcp.rs` | Remove the two `reg!("mcp.list_presets"/"mcp.install_preset", …)` lines. |
| `interfaces/webchat/src/api/mcp.rs` | Re-point `McpPresetApi::list` → `extensions.catalog` (filter kind=mcp, installed=false); `McpPresetApi::install` → `extensions.configure` + `extensions.install`. Map the existing `McpPresetInfo`/`McpPresetEnvVar`/`PresetInstallOutcome` onto the `extensions.*` wire shapes. |
| `interfaces/webchat/src/views/settings/mcp.rs` | No UI redesign — the "Recommended" cards/dialog now drive the re-pointed `McpPresetApi`. |

No new crates. No second async runtime. No platform-API access. (Tech-stack guardrails hold.)

**Projection note (multi-transport → single `install_spec`):** a `McpPreset` carries a *ranked* `transports` list (remote→stdio fallback); a Hub `ExtensionEntry` has *one* `install_spec`. The primer picks the **first** transport in the ranked list. The preset engine's runtime-fallback-across-transports is therefore **not preserved** in the Hub entry — an accepted consequence (the website likewise publishes one best transport per entry; NoRuntime (D8) still reports a missing runtime rather than failing silently).

---

## 7. Secret model

Unchanged from the Hub install path (`src/hub/install.rs::mcp_config_from_spec` + `src/hub/secrets.rs`): secret-flagged env → `SharedTokenManager::store_secret` under `field_key(Mcp, id, env)`, persisted as `{{secret:NAME}}` in the actor config; resolved at spawn by `src/mcp/manager/secret_resolver.rs`. **Routing official MCP through `run_install` upgrades it from plaintext (old preset engine) to vault-backed for free.** No parallel secret scheme.

---

## 8. Testing strategy

- **Unit — primer projection:** `McpPreset` → `ExtensionEntry` (id = `aleph-hub:<slug>`, Official, `requires_config` reflects `required_env`, install_spec built from the preferred transport; secret env → `EnvDecl{secret:true}`).
- **Unit — primer gate:** empty cache → primer writes `aleph-hub` rows; non-empty cache → primer is a no-op; `replace_source("aleph-hub", remote)` overwrites primer rows.
- **Unit — reconcile continuity:** an entry id `aleph-hub:volcengine-veimagex` + a live server `aleph_hub_volcengine_veimagex` → `mark_installed` flips `installed:true` (regression guard across the primer→remote id contract).
- **Unit — NoRuntime:** stdio spec whose runtime is absent → disclosure/run_install reports NoRuntime (not a persisted dead server).
- **Panel:** `cargo check -p aleph-panel --lib --target wasm32-unknown-unknown` on the re-pointed `McpPresetApi`/`settings/mcp.rs`.
- **Cargo discipline:** scope to `cargo test -p alephcore --lib` for the Rust units; at most one `cargo check` before merge. No full suite.

---

## 9. Redline / guardrail compliance

- **Single-source (2026-06-20):** primer is a cache primer into the *one* `aleph-hub` slot; no peer source, no local dedup, no local categorization/provenance inference. D1/D5 intact.
- **R3 (core minimalism) / R10 (thin harness):** net **deletion** (retire `plan_install`/`InstallPlan` + two RPCs + the dual UI path); the primer is static data projection, not curation logic.
- **R7 / P8 (LLM sovereignty):** pure plumbing; no inference, no regex intent parsing.
- **R8 (everything is a tool):** install stays a tool-driven flow via `extensions.install`.
- **Gateway auth:** `extensions.*` keep their existing tier gating; removing `mcp.list_presets/install_preset` removes surface, adds none. Verify no remaining caller during implementation.

---

## 10. Non-goals / follow-ons

- **Skills convergence / Plugins convergence** — reuse this exact pattern (bundled official content primed into the `aleph-hub` slot on cold start; install via the kind's existing engine; reconcile by id; D9 delete-and-re-fetch migration for pre-Hub installs). Separate specs. (Skills/plugins pre-Hub installs are more likely *genuinely* unprocessable than MCP's mild id-mismatch, so D9 carries more weight there.)
- **PyPI / fixed git refs for official MCP servers** — owned by the SiliconFlow / Aleph-mcp spec.
- **Settings ▸ MCP *manual CRUD* store** — owned by `2026-06-21-mcp-store-unification-design.md`.
- **Removing the official-MCP copies from the website catalog** — optional; harmless under Path C (they *are* the steady-state source). The user's call on the Hub repo.

---

## 11. Open questions (for spec review)

1. **Settings ▸ MCP "Recommended": re-point (D6, chosen — reuse the `ea307c45c` UI, in-context discovery) vs remove (centralize all discovery in the Hub view, less code)?**
2. **Primer gate signal:** "zero `aleph-hub` rows in cache" (chosen — simple) vs an explicit "never-successfully-fetched" flag (handles the edge where the website legitimately publishes an empty catalog)?
3. **Cross-repo id contract (§5):** confirm the website's official-MCP entry ids already equal `aleph-hub:<catalog.json id>`; if not, which side aligns?
4. **Migration depth (D9):** delete + re-install (chosen — clean, avoids vault re-keying, tiny affected set; cost = re-enter keys for those servers) vs reconcile-alias (`mark_installed` also matches the legacy raw-slug id → old installs show installed with no deletion / no key loss, but the server keeps its legacy id/shape and is not truly "Hub-为准")?
