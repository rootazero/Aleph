# Design — Restore MCP/Skills/Plugins to the "Extensions" settings group + reconcile Hub browse installed-state

- **Date:** 2026-06-21
- **Status:** Approved (design); pending spec review
- **Related:** `2026-06-20-aleph-hub-single-source-design.md` (the single-source teardown this builds on), memory `extensions-store-progress`
- **Redlines touched:** R4 (interface = pure I/O — keep matching logic server-side), R6 (one core, many channels), R10 (thin harness — no new abstraction layers)

## TL;DR (中文)

两件事：

1. **导航归位** — 把"设置"里的 **MCP / Plugins / Skills** 三个标签从 "Advanced" 组移回 "Extensions" 组（与 ACP 同组）。这是上一次重构时被"降级"到 Advanced 的，有一个测试把现状锁死，需一并反转。
2. **修复 Hub 浏览页的"已安装"状态** — 经核查：Hub 安装与三个设置页**共用同一后端存储**，所以"Hub 装了 → 设置页能看到""设置页删了 → Hub 的 *已安装列表* 实时反映"都已正常。唯一缺陷是 **Hub 浏览网格(browse grid)的卡片始终显示"未安装"**，因为 `extensions.catalog` 返回的条目 `installed` 恒为 `false`，从不与已安装集合比对。采用**方案 A（服务端调和）**：在 `handle_catalog` 内把每个目录条目与实际已安装项匹配，回填 `installed`/`enabled`。面板无需改结构。

## 1. Context & problem

The user reported that the panel-settings MCP/Skills/Plugins config "was deleted" during the Aleph Hub refactor and asked to (a) keep them and (b) sync them with the Hub bidirectionally.

Investigation of `HEAD` shows the premise is only partly accurate:

- The three settings pages were **not deleted**. `McpView` / `SkillsView` / `PluginsView` exist (`interfaces/webchat/src/views/settings/{mcp,skills,plugins}.rs`, 575 / 1057 / 434 lines), are routed (`app.rs:439-441`), and have full manual CRUD via their own RPCs.
- They were, however, **moved** from the "Extensions" nav group to the "Advanced" group.
- The Hub already shares one backend source of truth with these pages, so most of the requested "sync" already works. The one genuine defect is the Hub **browse grid** never showing accurate installed-state.

The user's two concrete asks (confirmed via Q&A):

1. Move MCP/Plugins/Skills back into the "Extensions" settings group.
2. Make the Hub↔settings data sync correct (fix what's broken).

## 2. Goals / non-goals

**Goals**

- G1. MCP/Plugins/Skills tabs appear under the **Extensions** settings group, not Advanced.
- G2. The Hub **browse grid** shows the correct `Installed` badge for items the user actually has, and flips back to not-installed when they are removed anywhere.
- G3. Keep all three settings pages as the "advanced manual config" surface (no functional change to them).
- G4. Preserve manual install of non-Hub extensions via the settings pages (already works; must not regress).

**Non-goals**

- N1. No version-diff / `update_available` computation.
- N2. No provenance-at-install hardening (Approach C) in this pass — name-based matching for Plugin/Skill is accepted. Documented as a known limitation + future option.
- N3. No change to the Hub install flow, trust gating, or secret pipeline.
- N4. No removal/merge of the top-level "Aleph Hub" page — Hub (discovery/install) and the settings pages (manual config) intentionally coexist.

## 3. Current state (evidence)

### 3.1 Settings nav (the move target)

`interfaces/webchat/src/components/settings_sidebar.rs`:

- `SettingsTab` enum already groups `Mcp, Plugins, Skills, Acp` under a `// Extensions` comment (lines 25-29) — but `SETTINGS_GROUPS` puts only `Acp` in `"Extensions"` (line 236-239) and demotes the other three into `"Advanced"` (lines 240-251).
- A test `mcp_plugins_skills_demoted_to_advanced` (lines 289-317) **asserts the demoted state** — it must be inverted.
- The `settings.groups.extensions` i18n label and the per-tab i18n labels already exist (lines 94-97, 202). No i18n additions needed.

### 3.2 Sync round-trip — what already works

Hub install writes into the same store the settings pages read:

| Kind | Hub install target | Settings-page read | Same store? |
|---|---|---|---|
| MCP | `mcp.add_server` → `~/.aleph/mcp_config.json` (`mcp/manager/actor.rs:459`) | `mcp_config.list` (same actor, `actor.rs:809`) | ✅ |
| Plugin | `marketplace.install_to_scope` → `~/.aleph/extensions/…` (`hub/install.rs:108-115`) | `plugins.list` (filesystem discovery) | ✅ |
| Skill | shared skill system | `skills.status` (same system) | ✅ |

`extensions.installed` (`hub`/`gateway/handlers/extensions/catalog.rs:59-88`) reconciles live across all three backends, so deletions are auto-detected there.

### 3.3 The defect

`handle_catalog` (`src/gateway/handlers/extensions/catalog.rs:24-56`) serializes each cached entry and only injects `source_label`; it never cross-references the installed set. Cached entries are hardcoded `installed: false` (`hub/hub_catalog.rs` `into_entry`). The browse card reads `entry.installed` (`interfaces/webchat/src/components/extensions/card.rs:30,67`). Result: **every browse card always shows not-installed.**

### 3.4 Identity mapping (basis for matching)

- **MCP — deterministic & exact.** Install assigns `id = mcp_server_id(entry.id)` = `entry.id` with `:`/`/` → `_` (`hub/install.rs:79-100`). The reconciled installed entry id is `local:mcp:{server.id}` (`hub/reconcile.rs:8,30`). So a catalog MCP entry's expected installed id is `format!("local:mcp:{}", mcp_server_id(&entry.id))`.
- **Plugin — by name.** Install uses `entry.name`; the reconciled `PluginRecord.id` comes from the discovered manifest. No deterministic id link → match by normalized name.
- **Skill — by name.** No `Skill` arm in `run_install`; reconciled id is `local:skill:{skill_id}` from SKILL.md. Match by normalized name.

## 4. Design

### WS1 — Nav move (trivial)

In `settings_sidebar.rs` `SETTINGS_GROUPS`:

- Set the `"Extensions"` group tabs to `[Mcp, Plugins, Skills, Acp]` (the three restored tabs first, ACP last — matches the `SettingsTab` enum declaration order).
- Remove those three from the `"Advanced"` group (leaving `Browser, Policies, Security, Execution`).

Invert the locking test (rename `mcp_plugins_skills_demoted_to_advanced` → `mcp_plugins_skills_in_extensions_group`): assert the three are in `"Extensions"` and **not** in `"Advanced"`.

No route, i18n, or page-logic changes.

### WS2 — Server-side reconcile in `handle_catalog` (Approach A)

**Step 1 — Extract a shared reconcile helper.** Refactor the body of `handle_installed` into:

```rust
/// Live-reconciled installed extensions across MCP / plugins / skills.
/// Best-effort: a failing backend is logged and skipped (never aborts).
pub async fn collect_installed(mcp: Option<McpManagerHandle>) -> Vec<ExtensionEntry>
```

`handle_installed` becomes a thin wrapper: `success(req.id, { extensions: collect_installed(mcp).await })`.

> Behavior note: the current `handle_installed` hard-errors if `mcp.list_servers()` fails; `collect_installed` instead logs a warning and skips (matching how it already treats plugin-load failure). This is a deliberate, minor robustness improvement so a flaky MCP actor cannot blank the catalog/installed views.

**Step 2 — Expose the MCP id derivation.** Change `fn mcp_server_id` in `hub/install.rs:79` to `pub(crate)` so `catalog.rs` reuses it (avoids drift between install-time and match-time id derivation).

**Step 3 — Reconcile inside `handle_catalog`.** New signature:

```rust
pub async fn handle_catalog(
    req: JsonRpcRequest,
    cache: Arc<CatalogCache>,
    mcp: Option<McpManagerHandle>,
) -> JsonRpcResponse
```

After `cache.query(&filter)`:

1. `let installed = collect_installed(mcp).await;`
2. Build two indices (keys are owned `String`; `ExtensionKind` is not `Hash`, so use `kind.as_str()`):
   - `installed_ids: HashSet<String>` of each `ie.id`.
   - `by_name: HashMap<String, bool>` keyed `format!("{}:{}", ie.kind.as_str(), ie.name.trim().to_lowercase())` → `ie.enabled`.
3. For each catalog entry `e` (taken by value/`mut`), before serializing:
   - **MCP:** `let key = format!("local:mcp:{}", mcp_server_id(&e.id));` if `installed_ids.contains(&key)` → `e.installed = true; e.enabled = <that entry's enabled>` (look it up; simplest is to find in `installed`).
   - **Plugin / Skill:** `let key = format!("{}:{}", e.kind.as_str(), e.name.trim().to_lowercase());` if `by_name.contains_key(&key)` → `e.installed = true; e.enabled = by_name[&key]`.
4. Serialize as today (still inject `source_label` from `e.via`).

**Step 4 — Update the registration site.** `src/bin/aleph-server/commands/start/builder/handlers/extensions.rs:24-27`: clone `mcp` into the `extensions.catalog` closure and pass it to `handle_catalog(req, cache, mcp)` (mirrors the `extensions.installed` registration two blocks down).

**Panel:** no struct changes — `api/extensions.rs:41-45` already deserializes `installed`/`enabled`; `card.rs:67` already branches on `installed`.

## 5. Files to change

| File | Change |
|---|---|
| `interfaces/webchat/src/components/settings_sidebar.rs` | WS1: move 3 tabs Advanced→Extensions; invert the locking test |
| `src/hub/install.rs` | WS2 S2: `mcp_server_id` → `pub(crate)` |
| `src/gateway/handlers/extensions/catalog.rs` | WS2 S1+S3: add `collect_installed`; reconcile inside `handle_catalog`; thin `handle_installed`; new `handle_catalog` signature; unit tests |
| `src/bin/aleph-server/commands/start/builder/handlers/extensions.rs` | WS2 S4: pass `mcp` into the `extensions.catalog` registration |

## 6. Tests

- **WS1 (`settings_sidebar.rs`):** `mcp_plugins_skills_in_extensions_group` — the three paths are in `"Extensions"`, absent from `"Advanced"`. Keep `clawhub_tab_is_removed`.
- **WS2 (`catalog.rs` unit tests):**
  - MCP catalog entry whose id derives to an installed server id → `installed == true`, `enabled` propagated.
  - MCP catalog entry with no matching server → `installed == false`.
  - Plugin/Skill catalog entry matched by case-insensitive name → `installed == true`.
  - `collect_installed` is best-effort (returns partial list when a backend is empty/unavailable). (Pure matching logic is unit-testable by constructing `ExtensionEntry` fixtures; full backend wiring is covered by the runtime check below.)

Verification budget: `cargo check -p alephcore --lib` once (per repo discipline). Panel compile: `cargo check -p aleph-panel --target wasm32-unknown-unknown` if the sidebar change needs confirming.

## 7. Runtime verification (e2e, manual — only when implementing)

Requires `just wasm` + server rebuild (panel is `rust_embed`-compiled). Then:

1. Settings sidebar shows **Extensions → MCP / Plugins / Skills** (+ ACP).
2. Install one MCP server + one plugin from the Hub.
3. Each appears on its settings page; the Hub **browse card** now shows `Installed`.
4. Delete each from the settings page; the browse card flips back to not-installed (and the Installed slide-in drops it).

## 8. Risks & limitations

- **R-1 (Plugin/Skill name collision):** two extensions sharing a normalized name could cross-mark. Low likelihood in a curated single-source catalog. Mitigation path = Approach C (provenance-at-install) if it ever bites.
- **R-2 (per-query backend cost):** `handle_catalog` now calls the three backends on each browse query. All calls are local (no network) — catalog stays offline-capable — and browse is a low-frequency UI surface. Acceptable.
- **R-3 (handle_installed error semantics):** changes from hard-fail to best-effort on MCP error (see WS2 S1 note). Intended.

## 9. Future (out of scope — Approach C)

Write the origin catalog-id into each installed artifact at install time (MCP config field / plugin record / skill entry) so Plugin/Skill matching becomes exact and provenance survives. Deferred; only pursue if name-matching proves insufficient.
