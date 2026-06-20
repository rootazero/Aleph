# Extensions Store P5 — Migration & i18n Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Finish the unified Extensions Store rollout in the Leptos panel: remove the now-subsumed ClawHub settings tab, demote the old per-type MCP/Plugins/Skills panels into the "Advanced" settings group, and localize the last six hardcoded strings in the new store UI.

**Architecture:** Three surgical, independent migrations in the `aleph-panel` crate (`interfaces/webchat/`). (1) Delete the self-contained ClawHub settings tab across its 4 source files + 2 locale files. (2) Move three `SettingsTab` array elements from the `"Extensions"` group to the `"Advanced"` group in the single `SETTINGS_GROUPS` constant. (3) Add 6 `leptos_i18n` keys to both locale JSONs and replace the 6 remaining raw string literals. No backend/core changes; the backend `gateway/handlers/clawhub.rs` stays as the Store Agent's long-tail substrate.

**Tech Stack:** Rust 1.92, Leptos 0.8 (CSR/WASM), `leptos_i18n` 0.6 (build-time codegen from `locales/{en,zh}.json`), Tailwind. Crate: `aleph-panel` (lib name `aleph_panel`).

## Global Constraints

Every task implicitly includes these. Values copied verbatim from the approved spec (`docs/superpowers/specs/2026-06-19-unified-extensions-store-design.md` §13 + Decision Log #2/#3) and verified against the live tree on 2026-06-20.

- **Scope of removal is the FRONTEND ClawHub settings menu only:** delete `SettingsTab::ClawHub` + `views/settings/clawhub.rs` (and its locale keys). **Do NOT touch** the backend `src/gateway/handlers/clawhub.rs` RPC handlers — ClawHub becomes a long-tail source the Store Agent reaches via `store_fetch_docs`. **`acp` is unrelated — leave it** (its tab, view, route, and the `"Extensions"` settings group it lives in all stay).
- **Demote, do not delete:** `views/settings/{mcp,plugins,skills}.rs` move into the existing **"Advanced"** settings group — kept for power users, no longer the primary path. The view files, routes, enum variants, `path()`/`i18n_label()`/`icon_svg()` arms are all UNCHANGED; only their placement in `SETTINGS_GROUPS` moves.
- **i18n via `leptos_i18n`:** add keys to **both** `locales/en.json` and `locales/zh.json` with **identical key structure** — `leptos_i18n`'s build-time codegen fails the build if a key exists in one locale but not the other. **Never edit `src/i18n.rs`** (it is `include!`-generated glue). New keys: `extensions.<area>.<name>`, snake_case leaves, ≤3 levels deep.
- **Surgical changes:** every changed line traces to ClawHub-removal, the Advanced demotion, or one of the six listed store strings. No drive-by edits, no reformatting, no "improving" adjacent code. The shared `"Collapse sidebar"` string in `mode_sidebar.rs:130–131` is **out of scope** (it is global chrome, not store UI, and the spec scopes P5 i18n to `nav.extensions` + store keys) — leave it.
- **Build / verify on the HOST target** (`cargo check`/`cargo test -p aleph-panel --lib`); the crate compiles and runs `#[test]`s on host even though its production artifact is `wasm32-unknown-unknown` (built separately via `just`). **Do NOT build `alephcore`** (no core changes in P5; it is memory-heavy and OOMs on parallel builds). **Do NOT touch** `aleph-desktop-macos`/`-linux`.
- **Repo conventions:** `docs/` is gitignored — force-add plan/docs with `git add -f`. Work happens on branch `feat/unified-extensions-store`. Attribution is disabled globally — **no `Co-Authored-By` trailer** on commits.

---

## Grounding corrections (live-code mapping vs. spec assumptions)

A read-only mapping of the live frontend produced these corrections; the tasks below already account for them:

1. **P3 already localized ~all store copy.** The spec line "add `nav.extensions` + store-UI keys" is mostly already done: `nav.extensions` exists, and the entire `extensions.*` namespace (`en.json`/`zh.json` lines 1290–1351, incl. nested `cat`/`kind`/`trust` objects) is present and wired via `t!`/`t_string!`. **Only 6 raw literals remain** (Task 3). Category/kind/trust display names are i18n-driven via `label_key` (Rust holds keys, not strings) — **do not touch them.**
2. **ClawHub is not a default/fallback tab.** The settings router fallback is `_ => ().into_any()` (`app.rs:462`), so no default needs re-pointing after removal.
3. **No `api::clawhub` orphan.** `clawhub.rs` calls backend RPC inline (no dedicated `src/api/clawhub` module); deleting the file orphans nothing. `lib.rs` has no `deny(warnings)`.
4. **`load_catalog` has TWO callers** that must be updated when threading `i18n`: `browse.rs:45` and `install_flow.rs:86`. (`model_picker.rs:98 load_catalog()` is a different, arg-less function — ignore it.)
5. **`SETTINGS_GROUPS` is the source of truth for grouping**, not the enum's `// comments`. The "Advanced" group currently holds only `Browser, Policies, Security, Execution` (RoutingRules/Search live in the AI group despite the enum comment).

---

## Whole-feature file map

| File | Task | Responsibility / change |
|---|---|---|
| `interfaces/webchat/src/components/settings_sidebar.rs` | T1, T2 | Remove ClawHub enum variant + 3 match arms + array element (T1); move Mcp/Plugins/Skills array elements Extensions→Advanced (T2); add `#[cfg(test)] mod tests` |
| `interfaces/webchat/src/views/settings/mod.rs` | T1 | Delete `pub mod clawhub;` + `pub use clawhub::ClawHubView;` |
| `interfaces/webchat/src/app.rs` | T1 | Remove `ClawHubView` import token + `/settings/clawhub` route arm |
| `interfaces/webchat/src/views/settings/clawhub.rs` | T1 | **Delete entire file** (440 lines) |
| `interfaces/webchat/locales/en.json` | T1, T3 | Delete `settings.tabs.clawhub` + `settings.clawhub` object (T1); add `extensions.error.*` + `extensions.trust.integrity_*` (T3) |
| `interfaces/webchat/locales/zh.json` | T1, T3 | Same as en.json with Chinese values |
| `interfaces/webchat/src/views/extensions/browse.rs` | T3 | Thread `i18n` into `load_catalog`; localize catalog-load error |
| `interfaces/webchat/src/views/extensions/installed.rs` | T3 | Thread `i18n` into `load_installed`; localize 3 errors (installed/toggle/remove) |
| `interfaces/webchat/src/components/extensions/install_flow.rs` | T3 | Update the second `load_catalog` call site (pass `i18n`) |
| `interfaces/webchat/src/components/extensions/trust_modal.rs` | T3 | Localize `integrity` + `sha256 ✓` labels |

---

## Task 1: Remove the ClawHub settings tab

**Files:**
- Modify: `interfaces/webchat/src/components/settings_sidebar.rs` (5 deletions + new test module)
- Modify: `interfaces/webchat/src/views/settings/mod.rs:7,31`
- Modify: `interfaces/webchat/src/app.rs:19,443`
- Delete: `interfaces/webchat/src/views/settings/clawhub.rs`
- Modify: `interfaces/webchat/locales/en.json` (delete 2 regions)
- Modify: `interfaces/webchat/locales/zh.json` (delete 2 regions)

**Interfaces:**
- Consumes: nothing from other tasks.
- Produces: a `SettingsTab` enum with no `ClawHub` variant; an `"Extensions"` settings group whose `tabs` no longer contains `ClawHub` (still contains `Mcp, Plugins, Skills, Acp` — Task 2 moves the first three). Test helper `all_tab_paths()` in `settings_sidebar::tests`, reused by Task 2.

- [ ] **Step 1: Write the failing test**

Append this module to the very end of `interfaces/webchat/src/components/settings_sidebar.rs` (after the `SETTINGS_GROUPS` constant, line 266):

```rust

#[cfg(test)]
mod tests {
    use super::*;

    /// Every tab path reachable from the sidebar navigation.
    fn all_tab_paths() -> Vec<&'static str> {
        SETTINGS_GROUPS
            .iter()
            .flat_map(|g| g.tabs.iter().map(|t| t.path()))
            .collect()
    }

    #[test]
    fn clawhub_tab_is_removed() {
        // ClawHub is subsumed into the Extensions store (P5); its settings tab must be gone.
        assert!(
            !all_tab_paths().contains(&"/settings/clawhub"),
            "ClawHub settings tab must be fully removed from SETTINGS_GROUPS"
        );
    }
}
```

This deliberately reads `path()` strings (stable) and never names `SettingsTab::ClawHub`, so it compiles both before and after the variant is deleted.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p aleph-panel --lib components::settings_sidebar::tests::clawhub_tab_is_removed`
Expected: FAIL — the assertion fires because `SettingsTab::ClawHub` is still in the `"Extensions"` group and its `path()` returns `/settings/clawhub`.

- [ ] **Step 3: Remove the ClawHub variant, match arms, and array element in `settings_sidebar.rs`**

These four deletions are one atomic edit — removing the variant without the match arms (or vice-versa) fails to compile.

(3a) Delete the enum variant — in `enum SettingsTab`, line 29:
```rust
    Skills,
    ClawHub,
    Acp,
```
becomes
```rust
    Skills,
    Acp,
```

(3b) Delete the `path()` arm (line 67):
```rust
            Self::Skills => "/settings/skills",
            Self::ClawHub => "/settings/clawhub",
            Self::Acp => "/settings/acp",
```
becomes
```rust
            Self::Skills => "/settings/skills",
            Self::Acp => "/settings/acp",
```

(3c) Delete the `i18n_label()` arm (line 99):
```rust
            Self::Skills => t_string!(i18n, settings.tabs.skills).to_string(),
            Self::ClawHub => t_string!(i18n, settings.tabs.clawhub).to_string(),
            Self::Acp => t_string!(i18n, settings.tabs.acp).to_string(),
```
becomes
```rust
            Self::Skills => t_string!(i18n, settings.tabs.skills).to_string(),
            Self::Acp => t_string!(i18n, settings.tabs.acp).to_string(),
```

(3d) Delete the `icon_svg()` arm (lines 153–155, three lines):
```rust
            Self::ClawHub => {
                r#"<path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><polyline points="7 10 12 15 17 10"/><line x1="12" y1="15" x2="12" y2="3"/>"#
            }
```
Delete those three lines entirely (the arm sits between `Self::Skills => { ... }` and `Self::Acp => { ... }`).

(3e) Delete the array element in the `"Extensions"` group of `SETTINGS_GROUPS` (line 248):
```rust
            SettingsTab::Mcp,
            SettingsTab::Plugins,
            SettingsTab::Skills,
            SettingsTab::ClawHub,
            SettingsTab::Acp,
```
becomes
```rust
            SettingsTab::Mcp,
            SettingsTab::Plugins,
            SettingsTab::Skills,
            SettingsTab::Acp,
```

- [ ] **Step 4: Remove the module + re-export in `views/settings/mod.rs`**

Delete line 7:
```rust
pub mod clawhub;
```
Delete line 31:
```rust
pub use clawhub::ClawHubView;
```

- [ ] **Step 5: Remove the import token + route arm in `app.rs`**

(5a) Line 19 — remove just the `ClawHubView,` token from the multi-name import:
```rust
    ChannelsOverview, ClawHubView, EmbeddingProvidersView, ExecutionView, GeneralView,
```
becomes
```rust
    ChannelsOverview, EmbeddingProvidersView, ExecutionView, GeneralView,
```

(5b) Line 443 — delete the route arm:
```rust
            "/settings/clawhub" => view! { <ClawHubView /> }.into_any(),
```

- [ ] **Step 6: Delete the component file**

```bash
git rm interfaces/webchat/src/views/settings/clawhub.rs
```
(The file is fully self-contained — `ClawHubView`, `ClawHubSkill`, helpers, `ClawHubSkillCard`. Nothing outside it imports `ClawHubSkill`; the only external references were the `pub use` in Step 4 and the import in Step 5.)

- [ ] **Step 7: Delete the ClawHub locale keys (both files)**

In **`interfaces/webchat/locales/en.json`**:

(7a) Delete the `settings.tabs.clawhub` label (line 394):
```json
      "skills": "Skills",
      "clawhub": "ClawHub",
      "acp": "ACP",
```
becomes
```json
      "skills": "Skills",
      "acp": "ACP",
```

(7b) Delete the entire `settings.clawhub` object (lines 878–903), so:
```json
      "loading": "Loading skills..."
    },
    "clawhub": {
      "title": "ClawHub",
      "description": "Browse and install from ClawHub marketplace",
      "search_placeholder": "Search skills...",
      "refresh": "Refresh",
      "search_results": "Search Results",
      "popular_skills": "Popular Skills",
      "no_skills": "No skills found",
      "no_skills_search_hint": "Try a different search query",
      "no_skills_browse_hint": "Use the search bar to find skills on ClawHub",
      "load_more": "Load more...",
      "loading": "Loading marketplace...",
      "no_results": "No results found",
      "install": "Install",
      "installing": "Installing...",
      "installed": "Installed",
      "update": "Update",
      "version": "Version",
      "author": "Author",
      "stars": "Stars",
      "type_plugin": "Plugin",
      "type_skill": "Skill",
      "all": "All",
      "plugins": "Plugins",
      "skills": "Skills"
    },
    "acp": {
```
becomes
```json
      "loading": "Loading skills..."
    },
    "acp": {
```

In **`interfaces/webchat/locales/zh.json`** (same lines, Chinese values):

(7c) Delete the tab label (line 394):
```json
      "skills": "技能",
      "clawhub": "ClawHub",
      "acp": "ACP",
```
becomes
```json
      "skills": "技能",
      "acp": "ACP",
```

(7d) Delete the `settings.clawhub` object (lines 878–903):
```json
      "loading": "加载技能..."
    },
    "clawhub": {
      "title": "ClawHub",
      "description": "浏览和安装 ClawHub 市场内容",
      "search_placeholder": "搜索技能...",
      "refresh": "刷新",
      "search_results": "搜索结果",
      "popular_skills": "热门技能",
      "no_skills": "未找到技能",
      "no_skills_search_hint": "尝试不同的搜索词",
      "no_skills_browse_hint": "使用搜索栏在 ClawHub 上查找技能",
      "load_more": "加载更多...",
      "loading": "加载市场...",
      "no_results": "未找到结果",
      "install": "安装",
      "installing": "安装中...",
      "installed": "已安装",
      "update": "更新",
      "version": "版本",
      "author": "作者",
      "stars": "星标",
      "type_plugin": "插件",
      "type_skill": "技能",
      "all": "全部",
      "plugins": "插件",
      "skills": "技能"
    },
    "acp": {
```
becomes
```json
      "loading": "加载技能..."
    },
    "acp": {
```

- [ ] **Step 8: Run the test to verify it passes + compile-check**

Run: `cargo test -p aleph-panel --lib components::settings_sidebar::tests::clawhub_tab_is_removed`
Expected: PASS.

Run: `cargo check -p aleph-panel --lib`
Expected: compiles with no errors (proves match exhaustiveness is satisfied, the deleted module/import/route resolve, and the `t_string!(i18n, settings.tabs.clawhub)` reference is gone so the now-deleted locale key is not referenced).

- [ ] **Step 9: Verify zero residual references**

Run: `git grep -in "clawhub" -- interfaces/webchat/src interfaces/webchat/locales`
Expected: **no output.** (Backend `src/gateway/handlers/clawhub.rs` is intentionally untouched and lives outside `interfaces/webchat/`.)

- [ ] **Step 10: Commit**

```bash
git add interfaces/webchat/src/components/settings_sidebar.rs \
        interfaces/webchat/src/views/settings/mod.rs \
        interfaces/webchat/src/app.rs \
        interfaces/webchat/locales/en.json \
        interfaces/webchat/locales/zh.json
git rm interfaces/webchat/src/views/settings/clawhub.rs
git commit -m "refactor(panel): remove ClawHub settings tab (subsumed into Extensions store)"
```

---

## Task 2: Demote MCP/Plugins/Skills to the Advanced group

**Files:**
- Modify: `interfaces/webchat/src/components/settings_sidebar.rs` (`SETTINGS_GROUPS` array only) + one new test

**Interfaces:**
- Consumes: from Task 1, the `"Extensions"` group's `tabs` array now contains `Mcp, Plugins, Skills, Acp` (ClawHub removed); the `all_tab_paths()` test helper.
- Produces: `"Advanced"` group containing `Browser, Policies, Security, Execution, Mcp, Plugins, Skills`; `"Extensions"` group containing only `Acp`. Enum/`path()`/`i18n_label()`/`icon_svg()` unchanged.

**Decision (recorded):** After demotion the `"Extensions"` settings group holds only `Acp`. We keep it (smallest change; ACP is conceptually an extension; spec says leave ACP alone). Mcp/Plugins/Skills are appended *after* the existing Advanced tabs to preserve their order.

- [ ] **Step 1: Write the failing test**

Add this test inside the existing `#[cfg(test)] mod tests` in `settings_sidebar.rs` (created in Task 1), after `clawhub_tab_is_removed`:

```rust

    /// Tab paths in the named settings group.
    fn group_tab_paths(label: &str) -> Vec<&'static str> {
        SETTINGS_GROUPS
            .iter()
            .find(|g| g.label == label)
            .map(|g| g.tabs.iter().map(|t| t.path()).collect())
            .unwrap_or_default()
    }

    #[test]
    fn mcp_plugins_skills_demoted_to_advanced() {
        let advanced = group_tab_paths("Advanced");
        assert!(advanced.contains(&"/settings/mcp"), "Advanced must contain MCP");
        assert!(advanced.contains(&"/settings/plugins"), "Advanced must contain Plugins");
        assert!(advanced.contains(&"/settings/skills"), "Advanced must contain Skills");

        let extensions = group_tab_paths("Extensions");
        assert!(!extensions.contains(&"/settings/mcp"), "Extensions must not contain MCP");
        assert!(!extensions.contains(&"/settings/plugins"), "Extensions must not contain Plugins");
        assert!(!extensions.contains(&"/settings/skills"), "Extensions must not contain Skills");
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p aleph-panel --lib components::settings_sidebar::tests::mcp_plugins_skills_demoted_to_advanced`
Expected: FAIL — the `"Advanced"` group does not yet contain `/settings/mcp`.

- [ ] **Step 3: Move the three tabs in `SETTINGS_GROUPS`**

Remove `Mcp, Plugins, Skills` from the `"Extensions"` group. The group:
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
```
becomes
```rust
    SettingsGroup {
        label: "Extensions",
        tabs: &[SettingsTab::Acp],
    },
```

Append them to the `"Advanced"` group. The group:
```rust
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
becomes
```rust
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

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p aleph-panel --lib components::settings_sidebar::tests`
Expected: PASS (both `clawhub_tab_is_removed` and `mcp_plugins_skills_demoted_to_advanced`).

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/components/settings_sidebar.rs
git commit -m "refactor(panel): demote MCP/Plugins/Skills settings panels to Advanced group"
```

---

## Task 3: Localize the six remaining store strings

**Files:**
- Modify: `interfaces/webchat/locales/en.json` (add `extensions.error.*` + `extensions.trust.integrity_*`)
- Modify: `interfaces/webchat/locales/zh.json` (same, Chinese)
- Modify: `interfaces/webchat/src/views/extensions/browse.rs`
- Modify: `interfaces/webchat/src/views/extensions/installed.rs`
- Modify: `interfaces/webchat/src/components/extensions/install_flow.rs`
- Modify: `interfaces/webchat/src/components/extensions/trust_modal.rs`

**Interfaces:**
- Consumes: nothing from T1/T2 (independent — different files/regions).
- Produces: no new public API. `load_catalog` gains an `i18n: I18nContext<Locale>` parameter; `load_installed` gains an `i18n: I18nContext<Locale>` parameter.

**Keys to add (both locales, identical structure):**

| Key | en | zh | Replaces |
|---|---|---|---|
| `extensions.error.catalog_load` | `Failed to load catalog` | `加载目录失败` | `browse.rs:27` |
| `extensions.error.installed_load` | `Failed to load installed` | `加载已安装列表失败` | `installed.rs:34` |
| `extensions.error.toggle_failed` | `Toggle failed` | `切换失败` | `installed.rs:140` |
| `extensions.error.remove_failed` | `Remove failed` | `移除失败` | `installed.rs:154` |
| `extensions.trust.integrity_label` | `Integrity` | `完整性` | `trust_modal.rs:56` |
| `extensions.trust.integrity_verified` | `sha256 ✓` | `sha256 ✓` | `trust_modal.rs:57` |

The four `error.*` values are `format!("{prefix}: {e}")` prefixes — the runtime `{e}` detail is appended in Rust, so the stored value has no trailing `: {e}`.

- [ ] **Step 1: Demonstrate the RED (typed-i18n build gate)**

Edit `trust_modal.rs` lines 55–58 to reference the not-yet-existing keys. Current:
```rust
                                    {d.sha256.clone().map(|_| view! {
                                        <span class="text-text-tertiary">"integrity"</span>
                                        <span class="text-success">"sha256 ✓"</span>
                                    })}
```
becomes
```rust
                                    {d.sha256.clone().map(|_| view! {
                                        <span class="text-text-tertiary">{t!(i18n, extensions.trust.integrity_label)}</span>
                                        <span class="text-success">{t!(i18n, extensions.trust.integrity_verified)}</span>
                                    })}
```
(`trust_modal.rs` already has `use crate::i18n::{t, use_i18n};` and `let i18n = use_i18n();` — no new imports.)

- [ ] **Step 2: Run check to verify it fails**

Run: `cargo check -p aleph-panel --lib`
Expected: FAIL — `leptos_i18n` codegen has no `extensions.trust.integrity_label` field; the `t!` macro expansion does not resolve.

- [ ] **Step 3: Add the keys to both locale files**

In **`interfaces/webchat/locales/en.json`**, extend the `extensions.trust` object and add an `extensions.error` object. Current (lines 1317–1323):
```json
    "trust": {
      "official": "Official",
      "verified": "Verified",
      "community": "Community",
      "unverified": "Unverified"
    },
    "featured": "Featured",
```
becomes
```json
    "trust": {
      "official": "Official",
      "verified": "Verified",
      "community": "Community",
      "unverified": "Unverified",
      "integrity_label": "Integrity",
      "integrity_verified": "sha256 ✓"
    },
    "error": {
      "catalog_load": "Failed to load catalog",
      "installed_load": "Failed to load installed",
      "toggle_failed": "Toggle failed",
      "remove_failed": "Remove failed"
    },
    "featured": "Featured",
```

In **`interfaces/webchat/locales/zh.json`**, current (lines 1317–1323):
```json
    "trust": {
      "official": "官方",
      "verified": "已验证",
      "community": "社区",
      "unverified": "未验证"
    },
    "featured": "精选",
```
becomes
```json
    "trust": {
      "official": "官方",
      "verified": "已验证",
      "community": "社区",
      "unverified": "未验证",
      "integrity_label": "完整性",
      "integrity_verified": "sha256 ✓"
    },
    "error": {
      "catalog_load": "加载目录失败",
      "installed_load": "加载已安装列表失败",
      "toggle_failed": "切换失败",
      "remove_failed": "移除失败"
    },
    "featured": "精选",
```

- [ ] **Step 4: Run check to verify the trust_modal change now passes**

Run: `cargo check -p aleph-panel --lib`
Expected: PASS — both new `trust.*` keys resolve in both locales.

- [ ] **Step 5: Localize the catalog-load error in `browse.rs`**

(5a) Update imports. Line 9:
```rust
use crate::i18n::{t, use_i18n};
```
becomes
```rust
use crate::i18n::{t, t_string, use_i18n, Locale};
use leptos_i18n::I18nContext;
```

(5b) Add the `i18n` parameter to `load_catalog`. Line 13:
```rust
pub(crate) fn load_catalog(state: DashboardState, store: StoreState, quiet: bool) {
```
becomes
```rust
pub(crate) fn load_catalog(state: DashboardState, store: StoreState, i18n: I18nContext<Locale>, quiet: bool) {
```

(5c) Localize the error. Lines 26–31:
```rust
            Err(e) => {
                store.error.set(Some(format!("Failed to load catalog: {e}")));
                if !quiet {
                    store.loading.set(false);
                }
            }
```
becomes
```rust
            Err(e) => {
                let prefix = t_string!(i18n, extensions.error.catalog_load).to_string();
                store.error.set(Some(format!("{prefix}: {e}")));
                if !quiet {
                    store.loading.set(false);
                }
            }
```

(5d) Update the in-component call site. Line 45:
```rust
            load_catalog(state, store, false);
```
becomes
```rust
            load_catalog(state, store, i18n, false);
```
(`i18n` is already bound at `browse.rs:41` via `let i18n = use_i18n();` and is `Copy`, so it is captured by the surrounding `Effect::new(move || …)`.)

- [ ] **Step 6: Update the second `load_catalog` caller in `install_flow.rs`**

Line 86:
```rust
            load_catalog(state, store, true);
```
becomes
```rust
            load_catalog(state, store, i18n, true);
```
(`install_flow.rs` already binds `let i18n = use_i18n();` at line 71 and imports `use_i18n`; no new imports needed — it only forwards `i18n`.)

- [ ] **Step 7: Localize the three errors in `installed.rs`**

(7a) Update imports. Line 15:
```rust
use crate::i18n::{t, use_i18n};
```
becomes
```rust
use crate::i18n::{t, t_string, use_i18n, Locale};
use leptos_i18n::I18nContext;
```

(7b) Add the `i18n` parameter to `load_installed`. Lines 19–24:
```rust
fn load_installed(
    state: DashboardState,
    items: RwSignal<Vec<ExtensionEntry>>,
    loading: RwSignal<bool>,
    error: RwSignal<Option<String>>,
) {
```
becomes
```rust
fn load_installed(
    state: DashboardState,
    items: RwSignal<Vec<ExtensionEntry>>,
    loading: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    i18n: I18nContext<Locale>,
) {
```

(7c) Localize the load error. Lines 33–36:
```rust
            Err(e) => {
                error.set(Some(format!("Failed to load installed: {e}")));
                loading.set(false);
            }
```
becomes
```rust
            Err(e) => {
                let prefix = t_string!(i18n, extensions.error.installed_load).to_string();
                error.set(Some(format!("{prefix}: {e}")));
                loading.set(false);
            }
```

(7d) Update the Effect call site. Line 53:
```rust
            load_installed(state, items, loading, error);
```
becomes
```rust
            load_installed(state, items, loading, error, i18n);
```
(`i18n` is bound at `installed.rs:46` in `InstalledPanel`.)

(7e) Localize the toggle error. Lines 139–143:
```rust
                Err(e) => {
                    error.set(Some(format!("Toggle failed: {e}")));
                    enabled.set(!new_val);
                    toggling.set(false);
                }
```
becomes
```rust
                Err(e) => {
                    let prefix = t_string!(i18n, extensions.error.toggle_failed).to_string();
                    error.set(Some(format!("{prefix}: {e}")));
                    enabled.set(!new_val);
                    toggling.set(false);
                }
```
(`InstalledRow` binds `let i18n = use_i18n();` at line 108; it is `Copy` and captured by the `on_toggle` move closure.)

(7f) Localize the remove error + update the nested `load_installed` call. Lines 151–158:
```rust
            match ExtensionsApi::uninstall(&state, id).await {
                Ok(()) => load_installed(state, items, loading, error),
                Err(e) => {
                    error.set(Some(format!("Remove failed: {e}")));
                    confirming.set(false);
                }
            }
```
becomes
```rust
            match ExtensionsApi::uninstall(&state, id).await {
                Ok(()) => load_installed(state, items, loading, error, i18n),
                Err(e) => {
                    let prefix = t_string!(i18n, extensions.error.remove_failed).to_string();
                    error.set(Some(format!("{prefix}: {e}")));
                    confirming.set(false);
                }
            }
```

- [ ] **Step 8: Build to verify all call sites + macros resolve**

Run: `cargo check -p aleph-panel --lib`
Expected: PASS — both `load_catalog` callers and both `load_installed` callers pass `i18n`; all six `t!`/`t_string!` macros resolve against both locales.

- [ ] **Step 9: Verify no raw literals remain + locale parity**

Run: `git grep -n "Failed to load catalog\|Failed to load installed\|Toggle failed\|Remove failed" -- interfaces/webchat/src`
Expected: **no output** (all four prefixes now come from i18n).

Run: `git grep -n '"integrity"\|"sha256' -- interfaces/webchat/src`
Expected: **no output** (the trust-modal literals are gone).

Locale parity is **already enforced by the gate**: `leptos_i18n`'s build-time codegen fails if a key exists in one locale but not the other, so the passing `cargo check` in Step 8 proves the en/zh key sets stayed in sync. As a quick confirmation that each new key landed in both files, check the counts match:

```bash
git grep -c "integrity_label\|integrity_verified\|catalog_load\|installed_load\|toggle_failed\|remove_failed" -- interfaces/webchat/locales/en.json interfaces/webchat/locales/zh.json
```
Expected: both files report `6`.

- [ ] **Step 10: Commit**

```bash
git add interfaces/webchat/locales/en.json \
        interfaces/webchat/locales/zh.json \
        interfaces/webchat/src/views/extensions/browse.rs \
        interfaces/webchat/src/views/extensions/installed.rs \
        interfaces/webchat/src/components/extensions/install_flow.rs \
        interfaces/webchat/src/components/extensions/trust_modal.rs
git commit -m "i18n(panel): localize remaining Extensions store error + integrity strings (en/zh)"
```

---

## Self-Review

**1. Spec coverage (§13 + Decision Log #3):**
- "Demote `views/settings/{mcp,plugins,skills}.rs` to Advanced management group" → **Task 2** (moves the three tabs into the `"Advanced"` group; view files untouched per the "kept for power users" intent). ✅
- "Remove the ClawHub settings menu (`SettingsTab::ClawHub`, `views/settings/clawhub.rs` route)" → **Task 1**. ✅
- "`acp` is unrelated — leave it" → Acp untouched in all tasks; remains in the `"Extensions"` group. ✅
- "Backend ClawHub becomes a long-tail source" → `gateway/handlers/clawhub.rs` deliberately not in scope. ✅
- "i18n: add `nav.extensions` + store-UI keys … reuse `leptos_i18n`" → `nav.extensions` and the store namespace already shipped in P3; **Task 3** closes the remaining 6 literals via `leptos_i18n`. ✅

**2. Placeholder scan:** No "TBD"/"handle appropriately"/"similar to". Every code step shows full before/after with exact line anchors. ✅

**3. Type consistency:**
- `load_catalog(state, store, i18n, quiet)` — definition (T3 5b) and both call sites (T3 5d browse.rs:45, T3 6 install_flow.rs:86) agree on the new arity/param type `I18nContext<Locale>`. ✅
- `load_installed(state, items, loading, error, i18n)` — definition (T3 7b) and both call sites (T3 7d:53, T3 7f:152) agree. ✅
- Key names are identical between the en/zh additions, the Rust `t!`/`t_string!` macro paths, and the Task-3 table (`extensions.error.{catalog_load,installed_load,toggle_failed,remove_failed}`, `extensions.trust.{integrity_label,integrity_verified}`). ✅
- Test helpers `all_tab_paths()` (T1) and `group_tab_paths()` (T2) live in one `mod tests` and use only `SETTINGS_GROUPS` + `path()` (stable across both edits). ✅

**4. Ordering / atomicity:** T1 before T2 (both edit the `"Extensions"` `tabs` array; each leaves a compiling state). T3 is independent (disjoint files/regions). Each task ends green and committed.
