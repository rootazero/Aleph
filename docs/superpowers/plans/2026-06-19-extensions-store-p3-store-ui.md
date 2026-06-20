# Extensions Store — P3 Store UI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship the user-facing Extensions Store as a top-level, full-screen Leptos mode: browse by functional category (featured strip + category shelves + responsive card grid), search/filter, a detail drawer, a trust-disclosure modal, a JSON-schema config wizard, an installed view, and the install state machine — all over the live `extensions.*` RPC façade built in P0–P2.

**Architecture:** A new top-level `PanelMode::Extensions` (full-screen takeover, grouped with Teams) renders a `views/extensions/` module. A typed `api/extensions.rs` client wraps the 9 live `extensions.*` JSON-RPC methods; pure view-model logic (`views/extensions/model.rs`) owns category facets, client-side filtering, featured/shelf grouping, and trust/kind→token-class maps; reusable panel primitives + net-new `components/json_schema_form.rs` render the screens. State is the codebase-standard `RwSignal + spawn_local + Effect` (no Leptos `Resource`), shared via an `ExtensionsView`-provided context.

**Tech Stack:** Rust + Leptos 0.8 (CSR/WASM, crate `aleph-panel`), Tailwind v4 (OKLCH `@theme` tokens), `leptos_i18n` 0.6 (en/zh, compile-time keys), JSON-RPC over WebSocket via `DashboardState::rpc_call`.

---

## Global Constraints

See INDEX → "Global Constraints" (umbrella naming, thin façade, reuse-don't-fork, trust rails, functional browse axis, cached catalog, repo conventions). P3-specific (spec §12 + dossier-verified panel facts):

- **Top-level mode, NOT a settings sub-tab.** Add `PanelMode::Extensions` (Decision #2, spec §12) with full-screen takeover, grouped adjacent to `Teams`; the store lives in `interfaces/webchat/src/views/extensions/` (sibling to `views/teams/`), NOT under `views/settings/`.
- **Browse axis = functional `ExtensionCategory` (primary); `kind` is a secondary badge + filter only** (Decision #13). 13 categories (snake_case wire values): `search developer data productivity writing communication knowledge files design automation finance utilities other`. "Featured" and "All" are UI-only pseudo-facets (not server categories).
- **Frontend state convention (mandatory — match the whole crate):** `RwSignal` + `leptos::task::spawn_local` + `Effect`; **never** `Resource`/`create_resource`. Gate every first fetch on `state.is_connected.get()` inside an `Effect` (else `rpc_call` returns `Err("Not connected")`). `DashboardState` is `Copy` — pass by value into closures/`spawn_local`/child components. Error type is always `String`, surfaced via an `error: RwSignal<Option<String>>` banner. Refresh = re-run the loader (no cache layer). Per-action busy signals (`installing`/`toggling`/`removing`) drive spinners + `disabled=` and reset on BOTH success and error. Optimistic toggles revert on error. Gate install/uninstall/toggle on `state.is_operator()`.
- **Tokens, never hex.** Use semantic Tailwind utilities only (`bg-surface`, `bg-surface-raised`, `bg-surface-sunken`, `bg-surface-overlay`, `text-text-{primary,secondary,tertiary,inverse}`, `border-border{,-subtle,-strong}`, `bg-primary`/`text-white`, `{success,warning,danger,info}{,-subtle}`, `rounded-{lg,xl,2xl,full}`, `shadow-{sm,md,xl}`, `.glass`, `.aleph-scrim`, `.aleph-content-top`). NEVER inline the mockup's warm hex (`#F6F3EC`/`#10665C`) — it would freeze in light mode and break dark/accent theming.
- **Serif display font:** add a `--font-serif: "Fraunces","Noto Sans SC",Georgia,serif;` token to `styles/tailwind.css` `@theme` and extend the Google-Fonts `<link>` in `index.html`; apply the generated `font-serif` utility to display headings (store title, category/shelf titles, card names, drawer/modal titles). Body stays `font-sans` (Inter); technical text (command, sha256, kind badges) stays `font-mono` (JetBrains Mono).
- **Trust/kind colors map to EXISTING tokens** (no new token blocks — avoids touching the light/`.dark`/system-mirror verbatim-copy invariant): trust `official→primary`, `verified→success`, `community→text-tertiary`, `unverified→warning`; risk banners `runs_commands→danger`, `remote_endpoint→warning`, `instructs_agent→warning`. Kind badge `skill→success`, `plugin→primary`, `mcp→info`.
- **i18n is compile-time-checked.** Every `t!`/`t_string!` key MUST exist in BOTH `locales/en.json` and `locales/zh.json` or the build fails. Therefore **each task that introduces UI copy adds its keys to BOTH locale files in the same task** under a new top-level `extensions` namespace (+ `nav.extensions`). (Reconciliation: the INDEX phasing lists "i18n" under P5; P5 covers the *migration* i18n — demoted panels, removed ClawHub. The store's own keys MUST land in P3 because the panel will not compile otherwise. Provide real zh translations, not English placeholders.)
- **RPC contract = the 9 live methods only** (see Reference §R2). Gap handling, locked:
  - **No detail-by-id / `CatalogParams` lacks `id`** → the detail drawer reuses the `ExtensionEntry` object the UI already holds from `extensions.catalog`; it does NOT re-fetch by id. Permissions come from `extensions.disclosure {id}`.
  - **No server-side trust filter** → filter by trust tier **client-side** on the returned `trust_tier`.
  - **No `extensions.sources.add`/`remove`** → P3 UI does NOT add/remove sources (out of the MVP closed loop, Decision #4). It MAY show source sync status via `extensions.sources.list` and a "Refresh catalog" action via `extensions.sources.refresh`. Do not invent add/remove UI.
  - **Config-form fields** come from `disclosure.secrets` (always available: `[{name,purpose,sensitive}]` = required||secret env fields) unioned with `entry.config_schema` (JSON Schema, when present, for type/default/placeholder). There is no `config_ui_hints` on the wire (the panel has none) — sensitivity comes from `disclosure.secrets[*].sensitive`.
  - **Lifecycle id namespace:** `extensions.toggle`/`extensions.uninstall` require an **installed** `local:{kind}:{backend}` id (from `extensions.installed`), NOT a catalog id. Never call toggle/uninstall with a catalog id.
- **Install gate order is backend-enforced** (`extensions.install`): ack → missing-fields → install → verify. The UI flow is driven by the install response branch (`needs_ack` → `{ok:false,missing}` → `{ok:true,...}`), never by the UI second-guessing the gate.
- **Test strategy:** pure logic (DTO/parse/filter/field-spec/reducer) → native `cargo test -p aleph-panel --lib <module>::tests`. Compile gate for view code → `cargo check -p aleph-panel --target wasm32-unknown-unknown` (the real build target; `trunk build` is the heavier alternative). Visual parity → manual run against the mockup. Per the build memory, scope test invocations to the touched module.
- **Repo:** `docs/` is gitignored → this plan is force-added (`git add -f`). Branch `feat/unified-extensions-store`. No attribution trailer in commits.

---

## Reference (verified — dossier-sourced, file:line exact)

### R1. Frontend RPC + state plumbing (`interfaces/webchat/src/`)
- `DashboardState::rpc_call(&self, method: &str, params: serde_json::Value) -> Result<serde_json::Value, String>` — `context.rs:441-479`. Already unwraps the JSON-RPC envelope: `Ok(result)` or `Err(error.message)`. The caller `serde_json::from_value`s `result`.
- Obtain context: `use crate::context::DashboardState; let state = expect_context::<DashboardState>();` (`mcp.rs:63`). `DashboardState` is `#[derive(Clone, Copy)]` (`context.rs:141-142`).
- Reactive gates on `state`: `is_connected: RwSignal<bool>`; `is_operator() -> bool` (`context.rs:318-321`).
- Imports every view needs: `use leptos::prelude::*; use leptos::task::spawn_local; use serde_json::json;`
- i18n: `use crate::i18n::{t, t_string, use_i18n, Locale}; let i18n = use_i18n();` then `{t!(i18n, extensions.key)}` (view text) / `t_string!(i18n, extensions.key).to_string()` (attrs/format). Interpolation: JSON `"{{ count }}"` + `t!(i18n, k, count = move || n)`.
- Typed API wrappers live in `interfaces/webchat/src/api/*.rs`, re-exported from `api.rs`. Template: `McpConfigApi` (`api/mcp.rs:27-94`). DTO derive header: `#[derive(Debug, Clone, Serialize, Deserialize)]` + `#[serde(default)]` on optional/new fields.

**Canonical load fn (copy this shape; from `mcp.rs:37-78`):**
```rust
fn load_catalog(
    state: DashboardState,
    entries: RwSignal<Vec<ExtensionEntry>>,
    loading: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    params: serde_json::Value,
) {
    loading.set(true);
    error.set(None);
    spawn_local(async move {
        match ExtensionsApi::catalog(&state, params).await {
            Ok(list) => { entries.set(list); loading.set(false); }
            Err(e) => { error.set(Some(format!("Failed to load catalog: {e}"))); loading.set(false); }
        }
    });
}
// Component: Effect::new(move || { if state.is_connected.get() { load_catalog(...) } else { loading.set(false); } });
```

**Canonical mutation (toggle, optimistic+revert; from `plugins.rs:291-309`):**
```rust
on:change=move |ev| {
    let new_val = event_target_checked(&ev);
    enabled.set(new_val);                 // optimistic
    toggling.set(true);
    spawn_local(async move {
        match ExtensionsApi::toggle(&state, id.clone(), new_val).await {
            Ok(()) => toggling.set(false),
            Err(e) => { error.set(Some(format!("Toggle failed: {e}"))); enabled.set(!new_val); toggling.set(false); }
        }
    });
}
```

### R2. Live `extensions.*` RPC (the only methods that exist)
| Method | Params (JSON) | Result |
|---|---|---|
| `extensions.catalog` | `{kind?, category?, source_id?, query?}` (all optional; may omit params) | `{ "extensions": ExtensionEntry[] }` |
| `extensions.installed` | none | `{ "extensions": ExtensionEntry[] }` (live reconciled; ids are `local:{kind}:{backend}`) |
| `extensions.disclosure` | `{id}` (catalog id) | `{ "disclosure": DisclosurePayload, "injection_findings": InjectionFinding[] }` |
| `extensions.configure` | `{id, values?}` | `{ "ok": bool, "missing": string[] }` |
| `extensions.install` | `{id, values?, acknowledge_risk?}` | branch — see R3 |
| `extensions.toggle` | `{id (local:), enabled}` | `{ "ok": true }` or RPC error |
| `extensions.uninstall` | `{id (local:)}` | `{ "ok": true }` or RPC error |
| `extensions.sources.list` | none | `{ "sources": [{id, trust_tier, kinds[]}] }` |
| `extensions.sources.refresh` | none | `{ "synced": [{source,count}], "failed": [{source,error}] }` |

`ExtensionEntry` wire (snake_case, `src/store/types.rs:147-176`): `id, kind("skill"|"plugin"|"mcp"), category(13 snake_case), name, description, author?(string), icon?, tags[], version?, source_id, trust_tier("official"|"verified"|"community"|"unverified"), repo_url?, requires_config(bool), config_schema?(JSON), installed(bool), enabled(bool), update_available(bool)`. Optionals are omitted-when-None. **No `config_ui_hints`, no `meta`, no `AuthorInfo` — `author` is a plain string.**

### R3. Install response branches (`extensions.install`)
1. `{ "ok": false, "needs_ack": true, "disclosure": DisclosurePayload, "injection_findings": [...] }`
2. `{ "ok": false, "missing": ["GITHUB_TOKEN", ...] }`
3. `{ "ok": true, "outcome": {kind,id|path}, "verify": {ok,tool_count?|error?}, "pin": {version?,sha256?}, "injection_findings": [...] }`

`DisclosurePayload` (`src/store/trust.rs:22-35`): `{ tier, risk("runs_commands"|"instructs_agent"|"remote_endpoint"), one_line, command_display?, secrets:[{name,purpose,sensitive}], version?, sha256?, ack_required(bool) }`. `ack_required = (risk == runs_commands) && tier ∈ {community,unverified}`. `InjectionFinding = {kind,detail}` (`kind ∈ zero_width|bidi_override|suspicious_phrase`).

### R4. Mode-wiring edit sites (`interfaces/webchat/src/`)
- `components/mode_sidebar.rs`: `PanelMode` enum `:21-30`; `from_path` `:32-50`; `ModeSidebar` match `:65-72` (exhaustive — must add arm).
- `components/nav_menu.rs`: `ALL_MODES: [PanelMode; 6]` `:17-24`; `route_of` `:27-36`; `label_of` `:39-48`; `icon_of` `:51-72` (all exhaustive matches). `use_navigate` already imported `:14`.
- `app.rs`: `MainContent` `:355-380` (per-mode `<div style:display=…>` — `==` compare, NOT exhaustive: must add manually); `use` block `:25`.
- `views/mod.rs`: flat `pub mod` list (add `pub mod extensions;`).
- Teams template to copy: `views/teams/mod.rs:50-93` (`TeamsView`), `:96-136` (`TeamsSidebar`), `:43-48` (`TeamsTabState` context struct of `RwSignal`s).
- `locales/{en,zh}.json`: `nav` block `:2-9`.

### R5. Reusable panel primitives (use these; don't re-invent)
- `crate::components::ui::{Card, CardHeader, CardContent, CardTitle, CardDescription}` (`components/ui/card.rs`); `Badge`/`BadgeVariant`/`StatusBadge` (`badge.rs`); `Button`/`ButtonVariant`/`ButtonSize` (`button.rs`); `ConfirmButton` (`confirm_button.rs`); `SecretInput` (`secret_input.rs`); `TagListInput` (`tag_list_input.rs`); `ChannelStatusPill` (`channel_status.rs`).
- `crate::components::forms::{SettingsSection, FormField, TextInput, SelectInput, NumberInput, SwitchInput, ErrorMessageDynamic, SaveButton}` (`components/forms.rs`).
- **Net-new (confirmed absent):** shared `Modal`/`Drawer`/`Tabs`/`EmptyState`/`Spinner` (inline the scaffolds in R6), and `json_schema_form` (Task 7).

### R6. Verified markup scaffolds (token-correct; transcribe as needed)
- **Page shell:** `<div class="flex-1 px-6 pb-6 overflow-y-auto bg-surface aleph-content-top"><div class="max-w-5xl space-y-6"> … </div></div>` (store uses `max-w-5xl` for the gallery grid).
- **Spinner:** `<div class="animate-spin rounded-full h-8 w-8 border-b-2 border-primary"></div>` centered in `<div class="flex items-center justify-center py-12">`.
- **Empty state:** `<div class="text-center py-12 border border-dashed border-border rounded-xl"><div class="text-4xl mb-4">{emoji}</div><p class="text-text-secondary">{msg}</p></div>`.
- **Error banner:** `<div class="p-3 bg-danger-subtle border border-border rounded text-danger text-sm">{err}</div>`.
- **Card row scaffold:** `<div class="p-4 bg-surface-raised border border-border rounded-xl hover:border-primary/40 transition-all">…</div>`; icon tile `<div class="w-10 h-10 rounded-lg bg-primary-subtle flex items-center justify-center flex-shrink-0">{glyph}</div>`.
- **Centered modal (install/config dialogs):**
  ```rust
  <Show when=move || open.get()>
    <div class="aleph-scrim fixed inset-0 bg-black/50 flex items-center justify-center z-50">
      <div class="glass bg-surface-overlay/85 border border-border rounded-xl w-full max-w-lg mx-4 max-h-[85vh] flex flex-col overflow-hidden">
        <div class="p-4 border-b border-border"> … head … </div>
        <div class="p-4 overflow-y-auto space-y-5 flex-1"> … body … </div>
        <div class="p-4 border-t border-border flex gap-2"> … foot … </div>
      </div>
    </div>
  </Show>
  ```
- **Right slide-over drawer (detail):**
  ```rust
  <div class="fixed inset-0 z-40 flex justify-end">
    <div class="aleph-scrim absolute inset-0 bg-black/30" on:click=close></div>
    <aside class="glass relative w-[480px] max-w-[94vw] h-full bg-surface-overlay/85 border-l border-border shadow-xl flex flex-col">
      <header class="px-4 py-3 border-b border-border flex items-start justify-between gap-2"> … </header>
      <div class="flex-1 overflow-y-auto p-4 space-y-4 text-sm"> … </div>
      <footer class="px-4 py-3 border-t border-border flex gap-2"> … </footer>
    </aside>
  </div>
  ```
- **Inline toggle:** `<label class="relative inline-flex items-center cursor-pointer"><input type="checkbox" class="sr-only peer" prop:checked=… on:change=… /><div class="w-11 h-6 bg-surface-sunken rounded-full peer peer-checked:bg-primary peer-checked:after:translate-x-full after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:rounded-full after:h-5 after:w-5 after:transition-all"></div></label>`.
- **Section heading (small-caps):** `class="text-xs font-semibold text-text-tertiary uppercase tracking-wider"`.
- **Count pill:** `class="px-2 py-0.5 rounded-full text-xs font-medium bg-primary-subtle text-primary"`.

---

## Whole-phase file map

| File | Responsibility | Task |
|---|---|---|
| `interfaces/webchat/src/api/extensions.rs` (NEW) | typed DTOs + `ExtensionsApi` + `parse_install_result` | T1 |
| `interfaces/webchat/src/api.rs` (MOD) | `pub mod extensions; pub use extensions::*;` | T1 |
| `interfaces/webchat/src/views/extensions/model.rs` (NEW) | category facets, filters, featured/shelf grouping, trust/kind class maps | T2 |
| `interfaces/webchat/src/views/extensions/mod.rs` (NEW) | `ExtensionsView` + `ExtensionsSidebar` + `StoreState` context | T3 |
| `interfaces/webchat/src/components/mode_sidebar.rs` (MOD) | `PanelMode::Extensions` enum/from_path/sidebar arm | T3 |
| `interfaces/webchat/src/components/nav_menu.rs` (MOD) | ALL_MODES/route_of/label_of/icon_of | T3 |
| `interfaces/webchat/src/app.rs` (MOD) | MainContent div arm + `use` | T3 |
| `interfaces/webchat/src/views/mod.rs` (MOD) | `pub mod extensions;` | T3 |
| `interfaces/webchat/index.html` (MOD) + `styles/tailwind.css` (MOD) | Fraunces `<link>` + `--font-serif` token | T3 |
| `interfaces/webchat/locales/{en,zh}.json` (MOD) | `nav.extensions` + `extensions.*` namespace (grown per task) | T3–T9 |
| `interfaces/webchat/src/components/extensions/mod.rs` (NEW) | barrel for store components | T4 |
| `interfaces/webchat/src/components/extensions/card.rs` (NEW) | `ExtensionCard` | T4 |
| `interfaces/webchat/src/views/extensions/browse.rs` (NEW) | catalog load + grid + chips + filters + search + featured/shelves | T4, T5 |
| `interfaces/webchat/src/components/extensions/detail_drawer.rs` (NEW) | `ExtensionDetailDrawer` + disclosure permissions | T6 |
| `interfaces/webchat/src/components/json_schema_form.rs` (NEW) | `FieldSpec`, `fields_from`, `JsonSchemaForm` | T7 |
| `interfaces/webchat/src/components/extensions/trust_modal.rs` (NEW) | `TrustModal` | T8 |
| `interfaces/webchat/src/components/extensions/install_flow.rs` (NEW) | install state machine + `next_step` reducer + leave-guard | T8 |
| `interfaces/webchat/src/views/extensions/installed.rs` (NEW) | installed slide-in: toggle/remove via `local:` ids | T9 |

**Natural split:** **P3a = T1–T3** (data + model + mode skeleton renders full-screen) — independently shippable. **P3b = T4–T6** (browse + drawer). **P3c = T7–T9** (config wizard + install flow + installed). Execute in order; each task ends green (compiles, pure tests pass) and is independently reviewable.

---

### Task 1: Typed `extensions` API client + DTOs

**Files:** Create `interfaces/webchat/src/api/extensions.rs`; Modify `interfaces/webchat/src/api.rs` (add `pub mod extensions;` to the module list and `pub use extensions::*;` to the glob block). Test inline.

**Interfaces:**
- Produces (consumed by every later task): DTOs `ExtensionEntry`, `SecretDisclosure`, `DisclosurePayload`, `InjectionFinding`, `SourceInfo`; enum `InstallResult { NeedsAck{disclosure, injection_findings}, Missing{missing}, Done{outcome, verify, pin, injection_findings} }`; pure `parse_install_result(&Value) -> Result<InstallResult, String>`; unit struct `ExtensionsApi` with async fns `catalog(&DashboardState, Value) -> Result<Vec<ExtensionEntry>,String>`, `installed(&DashboardState) -> Result<Vec<ExtensionEntry>,String>`, `disclosure(&DashboardState, String) -> Result<(DisclosurePayload, Vec<InjectionFinding>),String>`, `install(&DashboardState, String, Value, bool) -> Result<InstallResult,String>`, `toggle(&DashboardState, String, bool) -> Result<(),String>`, `uninstall(&DashboardState, String) -> Result<(),String>`, `sources_list(&DashboardState) -> Result<Vec<SourceInfo>,String>`, `sources_refresh(&DashboardState) -> Result<Value,String>`.

- [ ] **Step 1: Write the failing tests** at the bottom of `interfaces/webchat/src/api/extensions.rs`

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn entry_deserializes_minimal_wire_shape() {
        // optionals (author/icon/version/repo_url/config_schema) omitted, matching the backend
        let v = json!({
            "id": "mcp-official:io.github.acme/foo",
            "kind": "mcp",
            "category": "developer",
            "name": "Foo",
            "description": "Does foo.",
            "tags": ["mcp", "developer"],
            "source_id": "mcp-official",
            "trust_tier": "community",
            "requires_config": true,
            "installed": false,
            "enabled": false,
            "update_available": false
        });
        let e: ExtensionEntry = serde_json::from_value(v).unwrap();
        assert_eq!(e.id, "mcp-official:io.github.acme/foo");
        assert_eq!(e.kind, "mcp");
        assert_eq!(e.category, "developer");
        assert_eq!(e.author, None);
        assert!(e.requires_config);
        assert_eq!(e.tags, vec!["mcp".to_string(), "developer".to_string()]);
    }

    #[test]
    fn parse_install_needs_ack_branch() {
        let v = json!({
            "ok": false, "needs_ack": true,
            "disclosure": { "tier": "community", "risk": "runs_commands", "one_line": "Runs commands on your computer.",
                "command_display": "npx -y @x/y", "secrets": [{"name":"TOKEN","purpose":"auth","sensitive":true}], "ack_required": true },
            "injection_findings": []
        });
        match parse_install_result(&v).unwrap() {
            InstallResult::NeedsAck { disclosure, .. } => {
                assert_eq!(disclosure.risk, "runs_commands");
                assert!(disclosure.ack_required);
                assert_eq!(disclosure.secrets.len(), 1);
                assert!(disclosure.secrets[0].sensitive);
            }
            other => panic!("expected NeedsAck, got {other:?}"),
        }
    }

    #[test]
    fn parse_install_missing_branch() {
        let v = json!({ "ok": false, "missing": ["GITHUB_TOKEN", "ACCOUNT"] });
        match parse_install_result(&v).unwrap() {
            InstallResult::Missing { missing } => assert_eq!(missing, vec!["GITHUB_TOKEN".to_string(), "ACCOUNT".to_string()]),
            other => panic!("expected Missing, got {other:?}"),
        }
    }

    #[test]
    fn parse_install_done_branch() {
        let v = json!({ "ok": true, "outcome": {"kind":"mcp","id":"foo"},
            "verify": {"ok": true, "tool_count": 7}, "pin": {"version":"1.0.0","sha256":null}, "injection_findings": [] });
        match parse_install_result(&v).unwrap() {
            InstallResult::Done { verify, .. } => assert_eq!(verify.get("tool_count").and_then(|x| x.as_u64()), Some(7)),
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[test]
    fn parse_install_unknown_is_error() {
        assert!(parse_install_result(&json!({"weird": 1})).is_err());
    }
}
```

- [ ] **Step 2: Run → FAIL** `cargo test -p aleph-panel --lib api::extensions::tests` — expected: module not found / types undefined.

- [ ] **Step 3: Implement** the DTOs, parser, and API wrapper at the top of `interfaces/webchat/src/api/extensions.rs`

```rust
//! Typed client for the `extensions.*` JSON-RPC façade (P0–P2 backend).
//! Mirrors the exact wire shapes in `src/store/types.rs` / `src/store/trust.rs`.
use serde::Deserialize;
use serde_json::{json, Value};

use crate::context::DashboardState;

/// One catalog/installed entry. Wire shape: snake_case, optionals omitted-when-None.
/// `kind`/`category`/`trust_tier` are kept as `String` (forward-compatible with the open
/// category set — a new backend category must not break the panel deserializer).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ExtensionEntry {
    pub id: String,
    pub kind: String,
    pub category: String,
    pub name: String,
    #[serde(default)] pub description: String,
    #[serde(default)] pub author: Option<String>,
    #[serde(default)] pub icon: Option<String>,
    #[serde(default)] pub tags: Vec<String>,
    #[serde(default)] pub version: Option<String>,
    #[serde(default)] pub source_id: String,
    pub trust_tier: String,
    #[serde(default)] pub repo_url: Option<String>,
    #[serde(default)] pub requires_config: bool,
    #[serde(default)] pub config_schema: Option<Value>,
    #[serde(default)] pub installed: bool,
    #[serde(default)] pub enabled: bool,
    #[serde(default)] pub update_available: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SecretDisclosure {
    pub name: String,
    #[serde(default)] pub purpose: String,
    #[serde(default)] pub sensitive: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct DisclosurePayload {
    pub tier: String,
    pub risk: String,
    pub one_line: String,
    #[serde(default)] pub command_display: Option<String>,
    #[serde(default)] pub secrets: Vec<SecretDisclosure>,
    #[serde(default)] pub version: Option<String>,
    #[serde(default)] pub sha256: Option<String>,
    #[serde(default)] pub ack_required: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct InjectionFinding {
    pub kind: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SourceInfo {
    pub id: String,
    pub trust_tier: String,
    #[serde(default)] pub kinds: Vec<String>,
}

/// The three branch shapes of `extensions.install` (all are JSON-RPC successes).
#[derive(Debug, Clone, PartialEq)]
pub enum InstallResult {
    NeedsAck { disclosure: DisclosurePayload, injection_findings: Vec<InjectionFinding> },
    Missing { missing: Vec<String> },
    Done { outcome: Value, verify: Value, pin: Value, injection_findings: Vec<InjectionFinding> },
}

/// Pure: classify an `extensions.install` result `Value` into its branch.
/// Order matters — `needs_ack` also carries `ok:false`, so test it first.
pub fn parse_install_result(v: &Value) -> Result<InstallResult, String> {
    if v.get("needs_ack").and_then(Value::as_bool) == Some(true) {
        let disclosure = serde_json::from_value(v.get("disclosure").cloned().unwrap_or(Value::Null))
            .map_err(|e| format!("bad disclosure: {e}"))?;
        let injection_findings =
            serde_json::from_value(v.get("injection_findings").cloned().unwrap_or(json!([]))).unwrap_or_default();
        return Ok(InstallResult::NeedsAck { disclosure, injection_findings });
    }
    match v.get("ok").and_then(Value::as_bool) {
        Some(false) => {
            let missing =
                serde_json::from_value(v.get("missing").cloned().unwrap_or(json!([]))).unwrap_or_default();
            Ok(InstallResult::Missing { missing })
        }
        Some(true) => Ok(InstallResult::Done {
            outcome: v.get("outcome").cloned().unwrap_or(Value::Null),
            verify: v.get("verify").cloned().unwrap_or(Value::Null),
            pin: v.get("pin").cloned().unwrap_or(Value::Null),
            injection_findings:
                serde_json::from_value(v.get("injection_findings").cloned().unwrap_or(json!([]))).unwrap_or_default(),
        }),
        None => Err("unrecognized install response".into()),
    }
}

pub struct ExtensionsApi;

impl ExtensionsApi {
    pub async fn catalog(state: &DashboardState, params: Value) -> Result<Vec<ExtensionEntry>, String> {
        let r = state.rpc_call("extensions.catalog", params).await?;
        let arr = r.get("extensions").cloned().unwrap_or(json!([]));
        serde_json::from_value(arr).map_err(|e| format!("parse catalog: {e}"))
    }

    pub async fn installed(state: &DashboardState) -> Result<Vec<ExtensionEntry>, String> {
        let r = state.rpc_call("extensions.installed", Value::Null).await?;
        let arr = r.get("extensions").cloned().unwrap_or(json!([]));
        serde_json::from_value(arr).map_err(|e| format!("parse installed: {e}"))
    }

    pub async fn disclosure(
        state: &DashboardState,
        id: String,
    ) -> Result<(DisclosurePayload, Vec<InjectionFinding>), String> {
        let r = state.rpc_call("extensions.disclosure", json!({ "id": id })).await?;
        let disclosure = serde_json::from_value(r.get("disclosure").cloned().unwrap_or(Value::Null))
            .map_err(|e| format!("parse disclosure: {e}"))?;
        let findings =
            serde_json::from_value(r.get("injection_findings").cloned().unwrap_or(json!([]))).unwrap_or_default();
        Ok((disclosure, findings))
    }

    pub async fn install(
        state: &DashboardState,
        id: String,
        values: Value,
        acknowledge_risk: bool,
    ) -> Result<InstallResult, String> {
        let r = state
            .rpc_call("extensions.install", json!({ "id": id, "values": values, "acknowledge_risk": acknowledge_risk }))
            .await?;
        parse_install_result(&r)
    }

    pub async fn toggle(state: &DashboardState, id: String, enabled: bool) -> Result<(), String> {
        state.rpc_call("extensions.toggle", json!({ "id": id, "enabled": enabled })).await.map(|_| ())
    }

    pub async fn uninstall(state: &DashboardState, id: String) -> Result<(), String> {
        state.rpc_call("extensions.uninstall", json!({ "id": id })).await.map(|_| ())
    }

    pub async fn sources_list(state: &DashboardState) -> Result<Vec<SourceInfo>, String> {
        let r = state.rpc_call("extensions.sources.list", Value::Null).await?;
        let arr = r.get("sources").cloned().unwrap_or(json!([]));
        serde_json::from_value(arr).map_err(|e| format!("parse sources: {e}"))
    }

    pub async fn sources_refresh(state: &DashboardState) -> Result<Value, String> {
        state.rpc_call("extensions.sources.refresh", Value::Null).await
    }
}
```

Then register in `interfaces/webchat/src/api.rs`: add `pub mod extensions;` to the `pub mod` list and `pub use extensions::*;` to the glob-re-export block (so `crate::api::{ExtensionsApi, ExtensionEntry, ...}` resolve — match how `mcp` is exported).

- [ ] **Step 4: Run → PASS (5 tests)** `cargo test -p aleph-panel --lib api::extensions::tests`
- [ ] **Step 5: Commit** `feat(panel): typed extensions.* API client + DTOs + install-branch parser`

---

### Task 2: Catalog view-model — facets, filters, featured/shelves, class maps (pure)

**Files:** Create `interfaces/webchat/src/views/extensions/model.rs`; the `pub mod model;` declaration is added in Task 3's `views/extensions/mod.rs` — for THIS task, temporarily expose it by adding a minimal `interfaces/webchat/src/views/extensions/mod.rs` containing only `pub mod model;` and register `pub mod extensions;` in `views/mod.rs` (Task 3 fleshes the module out). Test inline.

**Interfaces:**
- Produces (consumed by browse/card/drawer): `struct CategoryFacet { value: &'static str, label_key: &'static str, emoji: &'static str }`; `const CATEGORIES: &[CategoryFacet]` (the 13 functional categories, snake_case `value`s matching the wire); `struct Filters { category: String, kind: String, trust: String, query: String }` (`Default` = all/empty); `fn matches(&ExtensionEntry, &Filters) -> bool`; `fn apply_filters(&[ExtensionEntry], &Filters) -> Vec<ExtensionEntry>`; `fn featured_picks(&[ExtensionEntry], usize) -> Vec<ExtensionEntry>`; `fn group_into_shelves(&[ExtensionEntry]) -> Vec<(&'static str, Vec<ExtensionEntry>)>`; class/label maps `kind_badge_class(&str)`, `kind_label_key(&str)`, `trust_dot_class(&str)`, `trust_label_key(&str)`, `risk_banner_class(&str)`.

- [ ] **Step 1: Write the failing tests** at the bottom of `model.rs`

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::extensions::ExtensionEntry;

    fn e(id: &str, kind: &str, cat: &str, trust: &str, name: &str, desc: &str, tags: &[&str]) -> ExtensionEntry {
        ExtensionEntry {
            id: id.into(), kind: kind.into(), category: cat.into(), name: name.into(),
            description: desc.into(), author: None, icon: None,
            tags: tags.iter().map(|s| s.to_string()).collect(), version: None,
            source_id: "s".into(), trust_tier: trust.into(), repo_url: None,
            requires_config: false, config_schema: None, installed: false, enabled: false, update_available: false,
        }
    }

    #[test]
    fn matches_category_kind_trust_query() {
        let item = e("a", "mcp", "developer", "community", "GitHub", "Manage repos", &["git"]);
        // category facet
        assert!(matches(&item, &Filters { category: "developer".into(), ..Default::default() }));
        assert!(!matches(&item, &Filters { category: "data".into(), ..Default::default() }));
        // "featured"/"all" are pass-through category facets
        assert!(matches(&item, &Filters { category: "featured".into(), ..Default::default() }));
        assert!(matches(&item, &Filters { category: "all".into(), ..Default::default() }));
        // kind (secondary)
        assert!(matches(&item, &Filters { kind: "mcp".into(), ..Default::default() }));
        assert!(!matches(&item, &Filters { kind: "skill".into(), ..Default::default() }));
        // trust filtered CLIENT-SIDE (server has no trust filter)
        assert!(matches(&item, &Filters { trust: "community".into(), ..Default::default() }));
        assert!(!matches(&item, &Filters { trust: "official".into(), ..Default::default() }));
        // query over name OR description OR tags, case-insensitive
        assert!(matches(&item, &Filters { query: "github".into(), ..Default::default() }));
        assert!(matches(&item, &Filters { query: "REPOS".into(), ..Default::default() }));
        assert!(matches(&item, &Filters { query: "git".into(), ..Default::default() }));
        assert!(!matches(&item, &Filters { query: "zzz".into(), ..Default::default() }));
    }

    #[test]
    fn featured_prefers_official_verified_capped_sorted() {
        let items = vec![
            e("c", "mcp", "data", "community", "Zeta", "", &[]),
            e("a", "mcp", "data", "official", "Beta", "", &[]),
            e("b", "skill", "writing", "verified", "Alpha", "", &[]),
        ];
        let f = featured_picks(&items, 2);
        assert_eq!(f.len(), 2);
        // official+verified only, sorted by name → Alpha, Beta
        assert_eq!(f[0].name, "Alpha");
        assert_eq!(f[1].name, "Beta");
    }

    #[test]
    fn shelves_skip_empty_and_follow_category_order() {
        let items = vec![
            e("a", "mcp", "developer", "official", "A", "", &[]),
            e("b", "mcp", "search", "official", "B", "", &[]),
        ];
        let shelves = group_into_shelves(&items);
        // CATEGORIES order has search before developer → search shelf first
        assert_eq!(shelves[0].0, "search");
        assert_eq!(shelves[1].0, "developer");
        assert_eq!(shelves.len(), 2); // no empty shelves for the other 11 categories
    }

    #[test]
    fn class_maps_cover_all_known_values() {
        assert_eq!(trust_dot_class("official"), "bg-primary");
        assert_eq!(trust_dot_class("verified"), "bg-success");
        assert_eq!(trust_dot_class("community"), "bg-text-tertiary");
        assert_eq!(trust_dot_class("unverified"), "bg-warning");
        assert_eq!(kind_badge_class("skill"), "bg-success-subtle text-success");
        assert_eq!(kind_badge_class("plugin"), "bg-primary-subtle text-primary");
        assert_eq!(kind_badge_class("mcp"), "bg-info-subtle text-info");
        assert_eq!(risk_banner_class("runs_commands"), "bg-danger-subtle text-danger border-danger/30");
    }
}
```

- [ ] **Step 2: Run → FAIL** `cargo test -p aleph-panel --lib views::extensions::model::tests`

- [ ] **Step 3: Implement** `model.rs`

```rust
//! Pure view-model for the Extensions store: functional-category facets, client-side
//! filtering (the server has no trust filter and only name-substring search), featured
//! and per-category-shelf grouping, and trust/kind → design-token class maps.
use crate::api::extensions::ExtensionEntry;

pub struct CategoryFacet {
    pub value: &'static str,    // snake_case wire category
    pub label_key: &'static str, // i18n key under `extensions.cat`
    pub emoji: &'static str,
}

/// Primary browse taxonomy (spec §12). Order here is the shelf/chip display order.
pub const CATEGORIES: &[CategoryFacet] = &[
    CategoryFacet { value: "search",        label_key: "extensions.cat.search",        emoji: "🔍" },
    CategoryFacet { value: "developer",     label_key: "extensions.cat.developer",     emoji: "🛠" },
    CategoryFacet { value: "data",          label_key: "extensions.cat.data",          emoji: "🗄" },
    CategoryFacet { value: "productivity",  label_key: "extensions.cat.productivity",  emoji: "⚡" },
    CategoryFacet { value: "writing",       label_key: "extensions.cat.writing",       emoji: "✍" },
    CategoryFacet { value: "communication", label_key: "extensions.cat.communication", emoji: "💬" },
    CategoryFacet { value: "knowledge",     label_key: "extensions.cat.knowledge",     emoji: "📚" },
    CategoryFacet { value: "files",         label_key: "extensions.cat.files",         emoji: "📁" },
    CategoryFacet { value: "design",        label_key: "extensions.cat.design",        emoji: "🎨" },
    CategoryFacet { value: "automation",    label_key: "extensions.cat.automation",    emoji: "🔁" },
    CategoryFacet { value: "finance",       label_key: "extensions.cat.finance",       emoji: "💰" },
    CategoryFacet { value: "utilities",     label_key: "extensions.cat.utilities",     emoji: "🧰" },
    CategoryFacet { value: "other",         label_key: "extensions.cat.other",         emoji: "•" },
];

#[derive(Debug, Clone, PartialEq)]
pub struct Filters {
    pub category: String, // "featured" | "all" | one of CATEGORIES.value
    pub kind: String,     // "all" | "skill" | "plugin" | "mcp"
    pub trust: String,    // "all" | "official" | "verified" | "community" | "unverified"
    pub query: String,
}

impl Default for Filters {
    fn default() -> Self {
        Self { category: "featured".into(), kind: "all".into(), trust: "all".into(), query: String::new() }
    }
}

#[must_use]
pub fn matches(e: &ExtensionEntry, f: &Filters) -> bool {
    let cat_ok = f.category == "featured" || f.category == "all" || e.category == f.category;
    let kind_ok = f.kind == "all" || e.kind == f.kind;
    let trust_ok = f.trust == "all" || e.trust_tier == f.trust;
    let query_ok = f.query.trim().is_empty() || {
        let q = f.query.to_lowercase();
        e.name.to_lowercase().contains(&q)
            || e.description.to_lowercase().contains(&q)
            || e.tags.iter().any(|t| t.to_lowercase().contains(&q))
    };
    cat_ok && kind_ok && trust_ok && query_ok
}

#[must_use]
pub fn apply_filters(entries: &[ExtensionEntry], f: &Filters) -> Vec<ExtensionEntry> {
    entries.iter().filter(|e| matches(e, f)).cloned().collect()
}

/// v1 deterministic stand-in for the Store Agent's editorial picks (P4 replaces this):
/// Official+Verified tiers, sorted by name, capped.
#[must_use]
pub fn featured_picks(entries: &[ExtensionEntry], max: usize) -> Vec<ExtensionEntry> {
    let mut v: Vec<ExtensionEntry> = entries
        .iter()
        .filter(|e| e.trust_tier == "official" || e.trust_tier == "verified")
        .cloned()
        .collect();
    v.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    v.truncate(max);
    v
}

/// Group entries into per-category shelves in CATEGORIES order; skip empty categories.
#[must_use]
pub fn group_into_shelves(entries: &[ExtensionEntry]) -> Vec<(&'static str, Vec<ExtensionEntry>)> {
    CATEGORIES
        .iter()
        .filter_map(|c| {
            let items: Vec<ExtensionEntry> = entries.iter().filter(|e| e.category == c.value).cloned().collect();
            (!items.is_empty()).then_some((c.value, items))
        })
        .collect()
}

#[must_use]
pub fn kind_label_key(kind: &str) -> &'static str {
    match kind {
        "skill" => "extensions.kind.skill",
        "plugin" => "extensions.kind.plugin",
        "mcp" => "extensions.kind.mcp",
        _ => "extensions.kind.other",
    }
}

#[must_use]
pub fn kind_badge_class(kind: &str) -> &'static str {
    match kind {
        "skill" => "bg-success-subtle text-success",
        "plugin" => "bg-primary-subtle text-primary",
        "mcp" => "bg-info-subtle text-info",
        _ => "bg-surface-sunken text-text-secondary",
    }
}

#[must_use]
pub fn trust_label_key(tier: &str) -> &'static str {
    match tier {
        "official" => "extensions.trust.official",
        "verified" => "extensions.trust.verified",
        "community" => "extensions.trust.community",
        _ => "extensions.trust.unverified",
    }
}

#[must_use]
pub fn trust_dot_class(tier: &str) -> &'static str {
    match tier {
        "official" => "bg-primary",
        "verified" => "bg-success",
        "community" => "bg-text-tertiary",
        "unverified" => "bg-warning",
        _ => "bg-text-tertiary",
    }
}

#[must_use]
pub fn risk_banner_class(risk: &str) -> &'static str {
    match risk {
        "runs_commands" => "bg-danger-subtle text-danger border-danger/30",
        "remote_endpoint" | "instructs_agent" => "bg-warning-subtle text-warning border-warning/30",
        _ => "bg-info-subtle text-info border-info/30",
    }
}
```

Create the temporary `interfaces/webchat/src/views/extensions/mod.rs` with just `pub mod model;`, and add `pub mod extensions;` to `interfaces/webchat/src/views/mod.rs`.

- [ ] **Step 4: Run → PASS (4 tests)** `cargo test -p aleph-panel --lib views::extensions::model::tests`
- [ ] **Step 5: Commit** `feat(panel): extensions store view-model — facets, filters, featured/shelves, class maps`

---

### Task 3: Top-level `Extensions` mode skeleton + serif font token

**Files:** Modify `components/mode_sidebar.rs`, `components/nav_menu.rs`, `app.rs`, `views/mod.rs` (already has `pub mod extensions;` from T2); flesh out `views/extensions/mod.rs`; add `nav.extensions` + an `extensions` namespace seed to `locales/{en,zh}.json`; add `--font-serif` to `styles/tailwind.css` and Fraunces to `index.html`. No new unit tests (compile + manual).

**Interfaces:**
- Produces: `PanelMode::Extensions`; `crate::views::extensions::{ExtensionsView, ExtensionsSidebar}`; a context struct `StoreState` (a `#[derive(Clone, Copy)]` of `RwSignal`s) provided by `ExtensionsView` and consumed by browse/drawer/installed in later tasks: `{ entries: RwSignal<Vec<ExtensionEntry>>, loading: RwSignal<bool>, error: RwSignal<Option<String>>, filters_* signals, selected: RwSignal<Option<ExtensionEntry>>, show_installed: RwSignal<bool> }`.

- [ ] **Step 1: Add the enum variant + routing.** In `components/mode_sidebar.rs`: add `Extensions,` to `PanelMode` (`:23-30`); add `} else if path.starts_with("/extensions") { Self::Extensions` to `from_path` (`:35-49`); add to the `ModeSidebar` match (`:65-72`):
```rust
PanelMode::Extensions => view! { <crate::views::extensions::ExtensionsSidebar /> }.into_any(),
```

- [ ] **Step 2: Add nav metadata.** In `components/nav_menu.rs`: bump `ALL_MODES` to `[PanelMode; 7]` and insert `PanelMode::Extensions,` immediately after `PanelMode::Teams,` (`:17-24`); add arms to `route_of` (`PanelMode::Extensions => "/extensions",`), `label_of` (`PanelMode::Extensions => t_string!(i18n, nav.extensions).to_string(),`), and `icon_of`:
```rust
PanelMode::Extensions => {
    r#"<path d="M20.5 11H19V7a2 2 0 0 0-2-2h-4V3.5a2.5 2.5 0 0 0-5 0V5H4a2 2 0 0 0-2 2v3.8h1.5a2.2 2.2 0 1 1 0 4.4H2V19a2 2 0 0 0 2 2h3.8v-1.5a2.2 2.2 0 1 1 4.4 0V21H17a2 2 0 0 0 2-2v-4h1.5a2.5 2.5 0 0 0 0-5z"/>"#
}
```

- [ ] **Step 3: Add the MainContent full-screen div + import.** In `app.rs` `use` block (`:25` area) add `use crate::views::extensions::ExtensionsView;`. In `MainContent` (`:355-380`), add a div arm mimicking the Teams line:
```rust
<div style:display=move || if mode.get() == PanelMode::Extensions { "contents" } else { "none" }>
    <ExtensionsView />
</div>
```

- [ ] **Step 4: Add i18n keys to BOTH locales.** In `locales/en.json` add `"extensions": "Extensions"` to the `nav` block (`:2-9`), and a new top-level `"extensions": { ... }` namespace with the seed keys this skeleton uses plus the category/kind/trust label keys referenced by Task 2's class maps:
```jsonc
"extensions": {
  "title": "Extensions",
  "subtitle": "Curated by your Store Agent",
  "back_to_chat": "← Back to chat",
  "cat": { "search":"Search & Web","developer":"Developer","data":"Data & Databases","productivity":"Productivity",
    "writing":"Writing","communication":"Communication","knowledge":"Knowledge","files":"Files","design":"Design & Media",
    "automation":"Automation","finance":"Finance","utilities":"Utilities","other":"Other","featured":"Featured","all":"All" },
  "kind": { "skill":"Skill","plugin":"Plugin","mcp":"MCP","other":"Ext" },
  "trust": { "official":"Official","verified":"Verified","community":"Community","unverified":"Unverified" }
}
```
In `locales/zh.json` add `"extensions": "扩展"` to `nav`, and the mirror namespace with zh values (title `"扩展"`, subtitle `"由你的商店智能体策展"`, back `"← 返回聊天"`, cat: 搜索与网络/开发/数据与数据库/效率/写作/沟通/知识/文件/设计与媒体/自动化/财务/工具/其他/精选/全部, kind: 技能/插件/MCP/扩展, trust: 官方/已验证/社区/未验证). **Keys must match the EN tree exactly.**

- [ ] **Step 5: Add the serif font.** In `index.html` (the Google-Fonts `<link>` ~`:13-16`), append `Fraunces:opsz,wght@9..144,400;9..144,600&family=Noto+Sans+SC:wght@400;600` to the existing `family=` query (one `<link>` edit). In `styles/tailwind.css` inside the `@theme { … }` block (near the `--font-sans`/`--font-mono` lines `~68-74`), add:
```css
--font-serif: "Fraunces", "Noto Sans SC", Georgia, serif;
```
(This auto-generates the `font-serif` utility; no `.dark`/system-mirror change needed — it is not a color token.)

- [ ] **Step 6: Flesh out `views/extensions/mod.rs`** (replace the T2 stub; keep `pub mod model;`)

```rust
//! Top-level Extensions store mode (full-screen takeover, grouped with Teams).
pub mod model;

use leptos::prelude::*;
use leptos_router::hooks::use_navigate;

use crate::api::extensions::ExtensionEntry;
use crate::context::DashboardState;
use crate::i18n::{t, use_i18n};

/// Shared store state, provided by `ExtensionsView`, consumed by browse/drawer/installed.
#[derive(Clone, Copy)]
pub struct StoreState {
    pub entries: RwSignal<Vec<ExtensionEntry>>,
    pub loading: RwSignal<bool>,
    pub error: RwSignal<Option<String>>,
    pub category: RwSignal<String>,
    pub kind_filter: RwSignal<String>,
    pub trust_filter: RwSignal<String>,
    pub query: RwSignal<String>,
    pub selected: RwSignal<Option<ExtensionEntry>>,
    pub show_installed: RwSignal<bool>,
}

impl StoreState {
    fn new() -> Self {
        Self {
            entries: RwSignal::new(Vec::new()),
            loading: RwSignal::new(true),
            error: RwSignal::new(None),
            category: RwSignal::new("featured".to_string()),
            kind_filter: RwSignal::new("all".to_string()),
            trust_filter: RwSignal::new("all".to_string()),
            query: RwSignal::new(String::new()),
            selected: RwSignal::new(None),
            show_installed: RwSignal::new(false),
        }
    }
}

#[component]
#[must_use]
pub fn ExtensionsView() -> impl IntoView {
    let _state = expect_context::<DashboardState>();
    let i18n = use_i18n();
    let store = StoreState::new();
    provide_context(store);
    let navigate = use_navigate();

    view! {
        <div class="flex-1 flex flex-col h-full overflow-hidden bg-surface aleph-content-top">
            <header class="px-6 py-3 border-b border-border flex items-center gap-4">
                <button
                    class="text-sm text-text-secondary hover:text-text-primary transition-colors"
                    on:click=move |_| navigate("/chat", Default::default())
                >
                    {t!(i18n, extensions.back_to_chat)}
                </button>
                <div>
                    <h1 class="font-serif text-2xl text-text-primary leading-tight">{t!(i18n, extensions.title)}</h1>
                    <p class="text-xs text-text-tertiary">{t!(i18n, extensions.subtitle)}</p>
                </div>
            </header>
            // Browse pane is mounted here in Task 4/5; installed slide-in in Task 9.
            <div class="flex-1 overflow-y-auto px-6 pb-6">
                <div class="max-w-5xl mx-auto py-8 text-text-tertiary text-sm">
                    "Store browse loads here."
                </div>
            </div>
        </div>
    }
}

#[component]
#[must_use]
pub fn ExtensionsSidebar() -> impl IntoView {
    // Minimal secondary column; the store's own topbar (chips/search/installed) lives in the
    // main area per the mockup. Category quick-nav is added with browse in Task 5.
    view! { <div class="flex flex-col h-full"></div> }
}
```

- [ ] **Step 7: Compile gate** `cargo check -p aleph-panel --target wasm32-unknown-unknown` — expected: clean (the four exhaustive `match`es force every new arm; i18n keys resolve in both locales).
- [ ] **Step 8: Manual verify** (run the panel via the `run` skill / `trunk serve`): the bottom-left NavMenu shows an **Extensions** item next to Teams; clicking it takes over the full content area with the serif "Extensions" title + "← Back to chat"; clicking Back returns to chat. Toggle dark mode — surfaces/text follow tokens.
- [ ] **Step 9: Commit** `feat(panel): top-level Extensions mode skeleton + serif font token + nav i18n`

---

### Task 4: ExtensionCard + catalog load + responsive grid

**Files:** Create `interfaces/webchat/src/components/extensions/mod.rs` (barrel) + `interfaces/webchat/src/components/extensions/card.rs`; create `interfaces/webchat/src/views/extensions/browse.rs`; register `pub mod extensions;` in `components/mod.rs` and `pub mod browse;` in `views/extensions/mod.rs`; mount `<BrowsePane/>` in `ExtensionsView`. Add card-related i18n keys to both locales.

**Interfaces:**
- Consumes: `StoreState` (context), `ExtensionsApi::catalog`, `model::{apply_filters, kind_badge_class, kind_label_key, trust_dot_class, trust_label_key, Filters}`.
- Produces: `#[component] ExtensionCard(entry: ExtensionEntry)` (sets `store.selected = Some(entry)` on click or Install — the detail drawer in Task 6 consumes `selected`; the real install action is wired in Task 8); `#[component] BrowsePane()` (load + grid).

- [ ] **Step 1: `components/extensions/mod.rs`** = `pub mod card;` (grows each task). Register `pub mod extensions;` in `components/mod.rs`.

- [ ] **Step 2: `card.rs`** — the store card (R6 card scaffold + R5 class maps)

```rust
use leptos::prelude::*;

use crate::api::extensions::ExtensionEntry;
use crate::i18n::{t, use_i18n};
use crate::views::extensions::model::{kind_badge_class, kind_label_key, trust_dot_class, trust_label_key};
use crate::views::extensions::StoreState;

#[component]
#[must_use]
pub fn ExtensionCard(entry: ExtensionEntry) -> impl IntoView {
    let store = expect_context::<StoreState>();
    let i18n = use_i18n();
    let e = entry.clone();
    let select = move |_| store.selected.set(Some(e.clone()));

    let badge_cls = format!("px-1.5 py-0.5 rounded text-[10px] font-mono font-bold uppercase tracking-wider {}", kind_badge_class(&entry.kind));
    let glyph = entry.icon.clone().unwrap_or_else(|| entry.name.chars().next().map(|c| c.to_string()).unwrap_or_default());
    let author = entry.author.clone().unwrap_or_default();
    let installed = entry.installed;

    view! {
        <div
            class="p-4 bg-surface-raised border border-border rounded-xl hover:border-primary/40 hover:shadow-md transition-all cursor-pointer flex flex-col gap-2"
            on:click=select.clone()
        >
            <div class="flex items-start gap-3">
                <div class="w-10 h-10 rounded-lg bg-primary-subtle flex items-center justify-center flex-shrink-0 text-lg">{glyph}</div>
                <div class="min-w-0 flex-1">
                    <div class="flex items-center gap-2">
                        <span class="font-serif text-base text-text-primary truncate">{entry.name.clone()}</span>
                        <span class=badge_cls>{t!(i18n, [kind_label_key(&entry.kind)])}</span>
                    </div>
                    <p class="text-xs text-text-tertiary truncate">{author}</p>
                </div>
            </div>
            <p class="text-sm text-text-secondary line-clamp-2">{entry.description.clone()}</p>
            <div class="flex items-center gap-2 mt-1">
                <span class=format!("inline-block w-2 h-2 rounded-full {}", trust_dot_class(&entry.trust_tier))></span>
                <span class="text-xs text-text-tertiary">{t!(i18n, [trust_label_key(&entry.trust_tier)])}</span>
                <span class="flex-1"></span>
                {move || if installed {
                    view! { <span class="px-3 py-1 rounded-lg text-xs bg-success-subtle text-success">{t!(i18n, extensions.installed)}</span> }.into_any()
                } else {
                    view! { <button class="px-3 py-1 rounded-lg text-xs bg-primary text-white hover:bg-primary-hover" on:click=select.clone()>{t!(i18n, extensions.install)}</button> }.into_any()
                }}
            </div>
        </div>
    }
}
```
> Note the `t!(i18n, [dynamic_key])` bracket form for a runtime key string — confirm against `leptos_i18n` 0.6: if the macro requires a literal key path, replace these two call sites with a small `match`-on-`&str` that returns the localized `&str` via `t_string!` over the literal variants (`extensions.kind.skill` etc.). Implementer-verify and use whichever the macro supports; the keys all exist from Task 3.

- [ ] **Step 3: `browse.rs`** — load + grid (R1 load pattern; Task 5 adds chips/filters/featured)

```rust
use leptos::prelude::*;
use serde_json::json;

use crate::api::extensions::{ExtensionEntry, ExtensionsApi};
use crate::components::extensions::card::ExtensionCard;
use crate::context::DashboardState;
use crate::i18n::{t, use_i18n};
use crate::views::extensions::model::{apply_filters, Filters};
use crate::views::extensions::StoreState;

fn load_catalog(state: DashboardState, store: StoreState) {
    store.loading.set(true);
    store.error.set(None);
    spawn_local(async move {
        match ExtensionsApi::catalog(&state, json!({})).await {
            Ok(list) => { store.entries.set(list); store.loading.set(false); }
            Err(e) => { store.error.set(Some(format!("Failed to load catalog: {e}"))); store.loading.set(false); }
        }
    });
}

#[component]
#[must_use]
pub fn BrowsePane() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let store = expect_context::<StoreState>();
    let i18n = use_i18n();

    Effect::new(move || {
        if state.is_connected.get() { load_catalog(state, store); } else { store.loading.set(false); }
    });

    // Filtered view (Task 5 binds the chip/filter signals; here Filters reads them already).
    let filtered = move || {
        let f = Filters {
            category: store.category.get(),
            kind: store.kind_filter.get(),
            trust: store.trust_filter.get(),
            query: store.query.get(),
        };
        apply_filters(&store.entries.get(), &f)
    };

    view! {
        {move || store.error.get().map(|err| view! {
            <div class="p-3 bg-danger-subtle border border-border rounded text-danger text-sm mb-4">{err}</div>
        })}
        {move || {
            if store.loading.get() {
                view! { <div class="flex items-center justify-center py-12"><div class="animate-spin rounded-full h-8 w-8 border-b-2 border-primary"></div></div> }.into_any()
            } else if filtered().is_empty() {
                view! { <div class="text-center py-12 border border-dashed border-border rounded-xl"><div class="text-4xl mb-4">"🧩"</div><p class="text-text-secondary">{t!(i18n, extensions.empty)}</p></div> }.into_any()
            } else {
                view! {
                    <div class="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-3">
                        <For each=move || filtered() key=|e: &ExtensionEntry| e.id.clone()
                            children=move |e| view! { <ExtensionCard entry=e /> } />
                    </div>
                }.into_any()
            }
        }}
    }
}
```

- [ ] **Step 4: Mount + i18n.** In `views/extensions/mod.rs` add `pub mod browse;`, and replace the placeholder body in `ExtensionsView` with `<div class="flex-1 overflow-y-auto px-6 pb-6"><div class="max-w-5xl mx-auto py-6"><crate::views::extensions::browse::BrowsePane /></div></div>`. Add to both locales under `extensions`: `"install":"Install"`, `"installed":"Installed"`, `"empty":"No extensions match."` (zh: 安装 / 已安装 / 没有匹配的扩展).

- [ ] **Step 5: Compile gate** `cargo check -p aleph-panel --target wasm32-unknown-unknown` — clean.
- [ ] **Step 6: Manual verify:** the store shows a responsive grid of cards from the live catalog (kind badge, trust dot+label, Install/Installed). Clicking a card sets `selected` (no visible effect yet — drawer is Task 6). Resize → 1/2/3 columns.
- [ ] **Step 7: Commit** `feat(panel): extension card + catalog load + responsive grid`

---

### Task 5: Category chips + Type/Trust filters + search + featured/shelves

**Files:** Create `interfaces/webchat/src/components/extensions/chips.rs`; modify `views/extensions/browse.rs` (add the chip bar, segmented filters, debounced search, and the featured-strip + per-category-shelf layout when `category == "featured"`); register `pub mod chips;` in `components/extensions/mod.rs`. Add filter/search i18n keys to both locales.

**Interfaces:**
- Consumes: `StoreState` signals (`category`/`kind_filter`/`trust_filter`/`query`), `model::{CATEGORIES, featured_picks, group_into_shelves}`.
- Produces: `#[component] CategoryChips()`, `#[component] FilterSegs()`, `#[component] StoreSearch()`.

- [ ] **Step 1: `chips.rs`** — chip bar + segmented controls + search

```rust
use leptos::prelude::*;

use crate::i18n::{t, t_string, use_i18n};
use crate::views::extensions::model::CATEGORIES;
use crate::views::extensions::StoreState;

#[component]
#[must_use]
pub fn CategoryChips() -> impl IntoView {
    let store = expect_context::<StoreState>();
    let i18n = use_i18n();
    // "Featured" + the 13 functional categories. Active chip = inverted (ink bg).
    let chip = move |value: &'static str, label: AnyView, emoji: &'static str| {
        let active = move || store.category.get() == value;
        view! {
            <button
                class=move || if active() { "flex items-center gap-1 px-3 py-1.5 rounded-full text-sm bg-text-primary text-surface whitespace-nowrap" }
                            else { "flex items-center gap-1 px-3 py-1.5 rounded-full text-sm bg-surface-sunken text-text-secondary hover:text-text-primary whitespace-nowrap" }
                on:click=move |_| store.category.set(value.to_string())
            ><span>{emoji}</span><span>{label}</span></button>
        }
    };
    view! {
        <div class="flex gap-2 overflow-x-auto pb-2">
            {chip("featured", view!{ {t!(i18n, extensions.cat.featured)} }.into_any(), "★")}
            {CATEGORIES.iter().map(|c| chip(c.value, view!{ {t!(i18n, [c.label_key])} }.into_any(), c.emoji)).collect_view()}
        </div>
    }
}

#[component]
#[must_use]
pub fn FilterSegs() -> impl IntoView {
    let store = expect_context::<StoreState>();
    let i18n = use_i18n();
    let seg = move |sig: RwSignal<String>, value: &'static str, label: String| {
        let active = move || sig.get() == value;
        view! {
            <button
                class=move || if active() { "px-2.5 py-1 rounded-md text-xs font-mono bg-text-primary text-surface" }
                            else { "px-2.5 py-1 rounded-md text-xs font-mono text-text-secondary hover:text-text-primary" }
                on:click=move |_| sig.set(value.to_string())
            >{label}</button>
        }
    };
    view! {
        <div class="flex items-center gap-4">
            <div class="flex items-center gap-1 bg-surface-sunken rounded-lg p-1">
                {seg(store.kind_filter, "all", t_string!(i18n, extensions.cat.all).to_string())}
                {seg(store.kind_filter, "skill", t_string!(i18n, extensions.kind.skill).to_string())}
                {seg(store.kind_filter, "plugin", t_string!(i18n, extensions.kind.plugin).to_string())}
                {seg(store.kind_filter, "mcp", t_string!(i18n, extensions.kind.mcp).to_string())}
            </div>
            <div class="flex items-center gap-1 bg-surface-sunken rounded-lg p-1">
                {seg(store.trust_filter, "all", t_string!(i18n, extensions.cat.all).to_string())}
                {seg(store.trust_filter, "official", t_string!(i18n, extensions.trust.official).to_string())}
                {seg(store.trust_filter, "verified", t_string!(i18n, extensions.trust.verified).to_string())}
                {seg(store.trust_filter, "community", t_string!(i18n, extensions.trust.community).to_string())}
            </div>
        </div>
    }
}

#[component]
#[must_use]
pub fn StoreSearch() -> impl IntoView {
    let store = expect_context::<StoreState>();
    let i18n = use_i18n();
    // Filtering is in-memory + reactive, so the query signal can update on every input
    // (no network debounce needed; `apply_filters` is cheap over the cached list).
    view! {
        <input
            class="w-full max-w-md px-3 py-2 bg-surface-sunken border border-border rounded-lg text-sm text-text-primary placeholder:text-text-tertiary focus:outline-none focus:ring-2 focus:ring-primary/30"
            prop:value=move || store.query.get()
            placeholder=move || t_string!(i18n, extensions.search_placeholder).to_string()
            on:input=move |ev| store.query.set(event_target_value(&ev))
        />
    }
}
```

- [ ] **Step 2: Wire the chrome + featured/shelf layout in `browse.rs`.** Render, above the grid: `<StoreSearch/>`, `<CategoryChips/>`, `<FilterSegs/>`. Then branch the body: when `store.category.get() == "featured"` AND search/filters are at defaults, render the **featured strip** (`featured_picks(&entries, 3)` as larger cards) followed by **shelves** (`group_into_shelves(&filtered)` — each shelf: a `font-serif` title from the category label + a horizontal/wrapped grid of `<ExtensionCard>`); otherwise render the flat filtered grid from Task 4. Featured/shelf code:
```rust
{move || {
    let entries = store.entries.get();
    let featured_view = store.category.get() == "featured" && store.query.get().trim().is_empty()
        && store.kind_filter.get() == "all" && store.trust_filter.get() == "all";
    if featured_view {
        let featured = crate::views::extensions::model::featured_picks(&entries, 3);
        let shelves = crate::views::extensions::model::group_into_shelves(&entries);
        view! {
            <div class="space-y-8">
                {(!featured.is_empty()).then(|| view! {
                    <div>
                        <h2 class="font-serif text-lg text-text-primary mb-3">{t!(i18n, extensions.featured)}</h2>
                        <div class="grid grid-cols-1 md:grid-cols-3 gap-3">
                            <For each=move || featured.clone() key=|e: &ExtensionEntry| e.id.clone()
                                children=move |e| view! { <ExtensionCard entry=e /> } />
                        </div>
                    </div>
                })}
                {shelves.into_iter().map(|(cat, items)| view! {
                    <div>
                        <h2 class="font-serif text-lg text-text-primary mb-3">{cat}</h2>
                        <div class="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-3">
                            <For each=move || items.clone() key=|e: &ExtensionEntry| e.id.clone()
                                children=move |e| view! { <ExtensionCard entry=e /> } />
                        </div>
                    </div>
                }).collect_view()}
            </div>
        }.into_any()
    } else {
        // flat filtered grid (Task 4 body) ...
    }
}}
```
> Implementer-verify: the shelf title should be the localized category label, not the raw `cat` value — map `cat` → its `CATEGORIES` `label_key` and `t!`/`t_string!` it (the keys exist from Task 3). Keep the empty/loading branches from Task 4 wrapping this.

- [ ] **Step 3: i18n.** Add to both locales under `extensions`: `"featured":"Featured"`, `"search_placeholder":"Search by what you want to do…"` (zh: 精选 / 按你想做的事搜索…). Register `pub mod chips;`.

- [ ] **Step 4: Compile gate** `cargo check -p aleph-panel --target wasm32-unknown-unknown` — clean.
- [ ] **Step 5: Manual verify:** the chip bar drives the primary category browse; Type/Trust segments filter (trust filters client-side); search narrows by name/desc/tags live; "Featured" shows the featured strip + category shelves; picking a category chip shows that category's flat grid.
- [ ] **Step 6: Commit** `feat(panel): category chips + type/trust filters + search + featured/shelf layout`

---

### Task 6: Detail drawer + permission disclosure

**Files:** Create `interfaces/webchat/src/components/extensions/detail_drawer.rs`; mount `<ExtensionDetailDrawer/>` in `ExtensionsView` (renders when `store.selected.is_some()`); register `pub mod detail_drawer;`. Add drawer i18n keys to both locales.

**Interfaces:**
- Consumes: `StoreState.selected`, `ExtensionsApi::disclosure`, `model::{trust_dot_class, trust_label_key, kind_badge_class, kind_label_key, risk_banner_class}`.
- Produces: `#[component] ExtensionDetailDrawer()` (right slide-over per R6 drawer scaffold). The footer **Install** button calls `store.selected`-derived install (wired in Task 8 — for THIS task it is a button that will be connected to the install flow; keep it present but its `on:click` sets a `pending_install` marker the flow consumes, or is a no-op closure to be replaced in Task 8). Reuses the entry object already held (no re-fetch by id); permissions come from `disclosure`.

- [ ] **Step 1: `detail_drawer.rs`** — slide-over with hero, stat row, "What it does", "What it can reach"

```rust
use leptos::prelude::*;

use crate::api::extensions::{DisclosurePayload, ExtensionsApi, SecretDisclosure};
use crate::context::DashboardState;
use crate::i18n::{t, use_i18n};
use crate::views::extensions::model::{kind_badge_class, kind_label_key, risk_banner_class, trust_dot_class, trust_label_key};
use crate::views::extensions::StoreState;

#[component]
#[must_use]
pub fn ExtensionDetailDrawer() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let store = expect_context::<StoreState>();
    let i18n = use_i18n();

    let disclosure = RwSignal::new(Option::<DisclosurePayload>::None);
    let disc_loading = RwSignal::new(false);

    // Lazy-load disclosure when an entry is selected.
    Effect::new(move || {
        if let Some(entry) = store.selected.get() {
            disclosure.set(None);
            disc_loading.set(true);
            let id = entry.id.clone();
            spawn_local(async move {
                match ExtensionsApi::disclosure(&state, id).await {
                    Ok((d, _findings)) => { disclosure.set(Some(d)); disc_loading.set(false); }
                    Err(_) => { disc_loading.set(false); }
                }
            });
        }
    });

    let close = move |_| store.selected.set(None);

    view! {
        <Show when=move || store.selected.get().is_some()>
            {move || {
                let entry = store.selected.get().unwrap();
                let badge_cls = format!("px-1.5 py-0.5 rounded text-[10px] font-mono font-bold uppercase {}", kind_badge_class(&entry.kind));
                view! {
                    <div class="fixed inset-0 z-40 flex justify-end">
                        <div class="aleph-scrim absolute inset-0 bg-black/30" on:click=close></div>
                        <aside class="glass relative w-[480px] max-w-[94vw] h-full bg-surface-overlay/85 border-l border-border shadow-xl flex flex-col">
                            <header class="px-4 py-3 border-b border-border flex items-start justify-between gap-2">
                                <div class="flex items-center gap-3 min-w-0">
                                    <div class="w-12 h-12 rounded-lg bg-primary-subtle flex items-center justify-center text-xl flex-shrink-0">
                                        {entry.icon.clone().unwrap_or_else(|| entry.name.chars().next().map(|c| c.to_string()).unwrap_or_default())}
                                    </div>
                                    <div class="min-w-0">
                                        <div class="flex items-center gap-2"><span class="font-serif text-lg text-text-primary truncate">{entry.name.clone()}</span><span class=badge_cls>{t!(i18n, [kind_label_key(&entry.kind)])}</span></div>
                                        <p class="text-xs text-text-tertiary truncate">{entry.author.clone().unwrap_or_default()}</p>
                                    </div>
                                </div>
                                <button class="text-text-tertiary hover:text-text-primary" on:click=close>"✕"</button>
                            </header>
                            <div class="flex-1 overflow-y-auto p-4 space-y-4 text-sm">
                                // stat row
                                <div class="grid grid-cols-3 gap-2 py-2 border-y border-border-subtle text-center">
                                    <div><p class="text-xs text-text-tertiary uppercase tracking-wider">{t!(i18n, extensions.version)}</p><p class="font-mono">{entry.version.clone().unwrap_or_else(|| "—".into())}</p></div>
                                    <div><p class="text-xs text-text-tertiary uppercase tracking-wider">{t!(i18n, extensions.category_label)}</p><p>{entry.category.clone()}</p></div>
                                    <div><p class="text-xs text-text-tertiary uppercase tracking-wider">{t!(i18n, extensions.trust_label)}</p><p class="flex items-center justify-center gap-1"><span class=format!("inline-block w-2 h-2 rounded-full {}", trust_dot_class(&entry.trust_tier))></span>{t!(i18n, [trust_label_key(&entry.trust_tier)])}</p></div>
                                </div>
                                // what it does
                                <div><h3 class="text-xs font-semibold text-text-tertiary uppercase tracking-wider mb-1">{t!(i18n, extensions.what_it_does)}</h3><p class="text-text-secondary">{entry.description.clone()}</p></div>
                                // what it can reach (permissions from disclosure)
                                <div>
                                    <h3 class="text-xs font-semibold text-text-tertiary uppercase tracking-wider mb-1">{t!(i18n, extensions.what_it_reaches)}</h3>
                                    {move || if disc_loading.get() {
                                        view! { <p class="text-text-tertiary italic">{t!(i18n, extensions.loading_perms)}</p> }.into_any()
                                    } else if let Some(d) = disclosure.get() {
                                        view! {
                                            <div class="space-y-2">
                                                <div class=format!("p-2 rounded border text-xs {}", risk_banner_class(&d.risk))>{d.one_line.clone()}</div>
                                                {d.command_display.clone().map(|cmd| view! { <div class="font-mono text-xs bg-surface-sunken p-2 rounded break-all">{cmd}</div> })}
                                                {(!d.secrets.is_empty()).then(|| view! {
                                                    <ul class="space-y-1">
                                                        {d.secrets.iter().map(|s: &SecretDisclosure| view! {
                                                            <li class="text-xs text-text-secondary">"🔑 "{s.name.clone()}{(!s.purpose.is_empty()).then(|| format!(" — {}", s.purpose))}</li>
                                                        }).collect_view()}
                                                    </ul>
                                                })}
                                            </div>
                                        }.into_any()
                                    } else {
                                        view! { <p class="text-text-tertiary italic">{t!(i18n, extensions.no_perms)}</p> }.into_any()
                                    }}
                                </div>
                            </div>
                            <footer class="px-4 py-3 border-t border-border flex gap-2">
                                <button class="flex-1 px-4 py-2 bg-primary text-white rounded-lg hover:bg-primary-hover text-sm" on:click=move |_| { /* Task 8: start_install(entry) */ }>{t!(i18n, extensions.install)}</button>
                                {entry.repo_url.clone().map(|url| view! { <a class="px-4 py-2 bg-surface-sunken text-text-secondary rounded-lg text-sm" href=url target="_blank" rel="noopener">{t!(i18n, extensions.docs)}</a> })}
                            </footer>
                        </aside>
                    </div>
                }
            }}
        </Show>
    }
}
```
> Implementer-verify: the dynamic `t!(i18n, [key])` sites (kind/trust labels) follow the same macro-form decision as Task 4 Step 2. The Install button's `on:click` is intentionally inert here (replaced in Task 8). Render the FULL untruncated description/command (spec §11 injection-hardening: no truncation in the approval surface).

- [ ] **Step 2: Mount.** In `views/extensions/mod.rs` add `pub mod` nothing (it's a component), and in `ExtensionsView`'s outer container add `<crate::components::extensions::detail_drawer::ExtensionDetailDrawer />` as a sibling after the content div (so the slide-over overlays the store). Register `pub mod detail_drawer;` in `components/extensions/mod.rs`.

- [ ] **Step 3: i18n.** Add to both locales under `extensions`: `"version":"Version"`, `"category_label":"Category"`, `"trust_label":"Trust"`, `"what_it_does":"What it does"`, `"what_it_reaches":"What it can reach"`, `"loading_perms":"Checking permissions…"`, `"no_perms":"No special permissions."`, `"docs":"Docs ↗"` (zh: 版本/类别/信任/功能说明/可访问的资源/正在检查权限…/无特殊权限/文档 ↗).

- [ ] **Step 4: Compile gate** `cargo check -p aleph-panel --target wasm32-unknown-unknown` — clean.
- [ ] **Step 5: Manual verify:** clicking a card slides in the right drawer with hero/stat row/description and a "What it can reach" section populated from `extensions.disclosure` (red banner for stdio MCP "Runs commands…", secret list); ✕ / backdrop closes it.
- [ ] **Step 6: Commit** `feat(panel): extension detail drawer + permission disclosure`

---

### Task 7: `json_schema_form.rs` — field-spec builder (pure) + form component

**Files:** Create `interfaces/webchat/src/components/json_schema_form.rs`; register `pub mod json_schema_form;` in `components/mod.rs`. Pure tests inline for the builder.

**Interfaces:**
- Produces: `struct FieldSpec { name: String, label: String, help: String, required: bool, secret: bool, placeholder: String, default: Option<String>, field_type: FieldType }`; `enum FieldType { Text, Secret, Bool, Number, Select(Vec<String>) }`; pure `fn fields_from(config_schema: Option<&serde_json::Value>, secrets: &[SecretDisclosure], missing: &[String]) -> Vec<FieldSpec>`; `#[component] JsonSchemaForm(fields: Vec<FieldSpec>, values: RwSignal<serde_json::Map<String, Value>>)` rendering each field via existing primitives and writing into `values`.

- [ ] **Step 1: Write the failing tests** (the builder is the testable core)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::extensions::SecretDisclosure;
    use serde_json::json;

    fn sd(name: &str, sensitive: bool) -> SecretDisclosure {
        SecretDisclosure { name: name.into(), purpose: format!("{name} purpose"), sensitive }
    }

    #[test]
    fn builds_from_disclosure_secrets_when_no_schema() {
        let secrets = vec![sd("GITHUB_TOKEN", true), sd("ACCOUNT", false)];
        let missing = vec!["GITHUB_TOKEN".to_string()];
        let fields = fields_from(None, &secrets, &missing);
        assert_eq!(fields.len(), 2);
        let tok = fields.iter().find(|f| f.name == "GITHUB_TOKEN").unwrap();
        assert!(tok.secret);
        assert!(tok.required); // present in `missing`
        assert_eq!(tok.field_type, FieldType::Secret);
        let acct = fields.iter().find(|f| f.name == "ACCOUNT").unwrap();
        assert!(!acct.secret);
        assert_eq!(acct.field_type, FieldType::Text);
    }

    #[test]
    fn schema_enriches_label_default_placeholder_and_type() {
        let schema = json!({
            "type": "object",
            "required": ["REGION"],
            "properties": {
                "REGION": { "type": "string", "description": "AWS region", "default": "us-east-1", "enum": ["us-east-1","eu-west-1"] },
                "GITHUB_TOKEN": { "type": "string", "description": "Token" }
            }
        });
        let secrets = vec![sd("GITHUB_TOKEN", true)];
        let fields = fields_from(Some(&schema), &secrets, &[]);
        let region = fields.iter().find(|f| f.name == "REGION").unwrap();
        assert!(region.required);                       // in schema.required
        assert_eq!(region.default.as_deref(), Some("us-east-1"));
        assert_eq!(region.field_type, FieldType::Select(vec!["us-east-1".into(), "eu-west-1".into()]));
        let tok = fields.iter().find(|f| f.name == "GITHUB_TOKEN").unwrap();
        assert!(tok.secret);                            // secret from disclosure even when schema-typed string
        assert_eq!(tok.field_type, FieldType::Secret);
    }

    #[test]
    fn secret_flag_forces_secret_type_over_schema_string() {
        let schema = json!({ "type":"object","properties": { "KEY": { "type":"string" } } });
        let fields = fields_from(Some(&schema), &[sd("KEY", true)], &[]);
        assert_eq!(fields[0].field_type, FieldType::Secret);
    }
}
```

- [ ] **Step 2: Run → FAIL** `cargo test -p aleph-panel --lib components::json_schema_form::tests`

- [ ] **Step 3: Implement** the builder + component

```rust
//! Schema→widget form for the install config wizard. Net-new (no JSON-schema form
//! renderer existed). `fields_from` (pure) merges the always-available
//! `disclosure.secrets` with the optional `config_schema` (JSON Schema) into a flat
//! field list; the component dispatches each field to an existing primitive.
use leptos::prelude::*;
use serde_json::{Map, Value};

use crate::api::extensions::SecretDisclosure;
use crate::components::forms::{FormField, SelectInput, TextInput};
use crate::components::ui::SecretInput;

#[derive(Debug, Clone, PartialEq)]
pub enum FieldType {
    Text,
    Secret,
    Bool,
    Number,
    Select(Vec<String>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct FieldSpec {
    pub name: String,
    pub label: String,
    pub help: String,
    pub required: bool,
    pub secret: bool,
    pub placeholder: String,
    pub default: Option<String>,
    pub field_type: FieldType,
}

/// Build the form's fields. Field set = union of `disclosure.secrets` names and
/// `config_schema.properties` keys. Sensitivity comes from `secrets[*].sensitive`
/// (overriding schema type). `required` = name ∈ `missing` OR ∈ `schema.required`.
#[must_use]
pub fn fields_from(
    config_schema: Option<&Value>,
    secrets: &[SecretDisclosure],
    missing: &[String],
) -> Vec<FieldSpec> {
    let props = config_schema
        .and_then(|s| s.get("properties"))
        .and_then(Value::as_object);
    let schema_required: Vec<String> = config_schema
        .and_then(|s| s.get("required"))
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();

    // Ordered field names: disclosure.secrets first (the must-fill set), then any
    // extra schema properties not already covered.
    let mut names: Vec<String> = secrets.iter().map(|s| s.name.clone()).collect();
    if let Some(p) = props {
        for k in p.keys() {
            if !names.contains(k) {
                names.push(k.clone());
            }
        }
    }

    names
        .into_iter()
        .map(|name| {
            let secret_decl = secrets.iter().find(|s| s.name == name);
            let prop = props.and_then(|p| p.get(&name));
            let is_secret = secret_decl.map(|s| s.sensitive).unwrap_or(false);
            let required = missing.contains(&name) || schema_required.contains(&name);
            let help = prop
                .and_then(|p| p.get("description"))
                .and_then(Value::as_str)
                .map(String::from)
                .or_else(|| secret_decl.map(|s| s.purpose.clone()))
                .unwrap_or_default();
            let default = prop
                .and_then(|p| p.get("default"))
                .and_then(Value::as_str)
                .map(String::from);
            let placeholder = prop
                .and_then(|p| p.get("placeholder").or_else(|| p.get("valueHint")))
                .and_then(Value::as_str)
                .map(String::from)
                .unwrap_or_default();
            let enum_vals: Option<Vec<String>> = prop
                .and_then(|p| p.get("enum"))
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect());
            let schema_type = prop.and_then(|p| p.get("type")).and_then(Value::as_str).unwrap_or("string");
            let field_type = if is_secret {
                FieldType::Secret
            } else if let Some(opts) = enum_vals {
                FieldType::Select(opts)
            } else {
                match schema_type {
                    "boolean" => FieldType::Bool,
                    "integer" | "number" => FieldType::Number,
                    _ => FieldType::Text,
                }
            };
            FieldSpec { name: name.clone(), label: name, help, required, secret: is_secret, placeholder, default, field_type }
        })
        .collect()
}

#[component]
#[must_use]
pub fn JsonSchemaForm(fields: Vec<FieldSpec>, values: RwSignal<Map<String, Value>>) -> impl IntoView {
    // Seed defaults once.
    let seed = fields.clone();
    Effect::new(move || {
        values.update(|m| {
            for f in &seed {
                if let Some(def) = &f.default {
                    m.entry(f.name.clone()).or_insert_with(|| Value::String(def.clone()));
                }
            }
        });
    });

    view! {
        <div class="space-y-4">
            {fields.into_iter().map(|f| {
                let name = f.name.clone();
                let label = if f.required { format!("{} *", f.label) } else { f.label.clone() };
                let get = {
                    let name = name.clone();
                    move || values.get().get(&name).and_then(Value::as_str).unwrap_or_default().to_string()
                };
                let set = {
                    let name = name.clone();
                    move |v: String| values.update(|m| { m.insert(name.clone(), Value::String(v)); })
                };
                let widget = match f.field_type.clone() {
                    FieldType::Secret => view! { <SecretInput value=Signal::derive(get.clone()) on_change=set.clone() placeholder=Some(f.placeholder.clone()) monospace=true /> }.into_any(),
                    FieldType::Select(opts) => {
                        let pairs: Vec<(String, String)> = opts.iter().map(|o| (o.clone(), o.clone())).collect();
                        view! { <SelectInput value=Signal::derive(get.clone()) on_change=set.clone() options=pairs /> }.into_any()
                    }
                    _ => view! { <TextInput value=Signal::derive(get.clone()) on_change=set.clone() placeholder=f.placeholder.clone() input_type="text".to_string() monospace=false /> }.into_any(),
                };
                view! { <FormField label=label help_text=Some(f.help.clone())>{widget}</FormField> }
            }).collect_view()}
        </div>
    }
}
```
> Implementer-verify: the exact prop names/signatures of `SecretInput`/`SelectInput`/`TextInput`/`FormField` (R5 — `components/ui/secret_input.rs`, `components/forms.rs`). Adjust `Signal::derive`/`on_change` shapes to match their real signatures (e.g. `SelectInput::options: Vec<(&str,&str)>` vs `Vec<(String,String)>`). `FieldType::Bool`/`Number` fall through to text here (v1: MCP env values are strings; the wizard submits string values — the backend coerces). Keep them in the enum for completeness; do not add bespoke widgets unless a real schema needs them (YAGNI).

- [ ] **Step 4: Run → PASS (3 tests)** `cargo test -p aleph-panel --lib components::json_schema_form::tests`; then `cargo check -p aleph-panel --target wasm32-unknown-unknown` — clean.
- [ ] **Step 5: Commit** `feat(panel): json_schema_form — field-spec builder + schema→widget form`

---

### Task 8: Trust modal + install state machine + leave-guard

**Files:** Create `interfaces/webchat/src/components/extensions/trust_modal.rs` and `interfaces/webchat/src/components/extensions/install_flow.rs`; extend `StoreState` (in `views/extensions/mod.rs`) with install-flow signals; wire the card's Install (Task 4) and drawer's Install (Task 6) to `start_install`; mount the flow's modals in `ExtensionsView`. Register both modules. Add install i18n keys to both locales. Pure test for the `next_step` reducer.

**Interfaces:**
- Consumes: `ExtensionsApi::{disclosure, install}`, `parse_install_result`/`InstallResult`, `JsonSchemaForm`/`fields_from`, `DisclosurePayload`.
- Produces: `enum InstallStep { Hidden, Trust, Configure, Installing, Done, Failed }`; pure `fn next_step(result: &InstallResult) -> InstallStep`; `#[component] TrustModal()`; `#[component] InstallFlow()` (renders Trust + Configure modals + Installing/Done/Failed states, owns the state machine); a `start_install(entry)` entry point exposed via `StoreState` (e.g. `StoreState.install_target: RwSignal<Option<ExtensionEntry>>` set by card/drawer; the flow `Effect`-watches it).

- [ ] **Step 1: Extend `StoreState`** in `views/extensions/mod.rs` with:
```rust
    pub install_target: RwSignal<Option<ExtensionEntry>>,
    pub install_step: RwSignal<crate::components::extensions::install_flow::InstallStep>,
    pub disclosure: RwSignal<Option<crate::api::extensions::DisclosurePayload>>,
    pub config_values: RwSignal<serde_json::Map<String, serde_json::Value>>,
    pub installing: RwSignal<bool>,
    pub install_error: RwSignal<Option<String>>,
```
(init in `StoreState::new`: target `None`, step `InstallStep::Hidden`, disclosure `None`, values `Map::new()`, installing `false`, error `None`.) Add `pub fn start_install(self, entry: ExtensionEntry)` that sets `install_target = Some(entry)`, `config_values = {}`, `install_error = None`, and kicks the flow (the `InstallFlow` Effect reacts).

- [ ] **Step 2: `install_flow.rs` reducer test (failing)**
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::extensions::{DisclosurePayload, InstallResult};

    fn disc(ack: bool) -> DisclosurePayload {
        DisclosurePayload { tier: "community".into(), risk: "runs_commands".into(), one_line: "x".into(),
            command_display: None, secrets: vec![], version: None, sha256: None, ack_required: ack }
    }

    #[test]
    fn needs_ack_goes_to_trust() {
        let r = InstallResult::NeedsAck { disclosure: disc(true), injection_findings: vec![] };
        assert_eq!(next_step(&r), InstallStep::Trust);
    }
    #[test]
    fn missing_goes_to_configure() {
        let r = InstallResult::Missing { missing: vec!["TOKEN".into()] };
        assert_eq!(next_step(&r), InstallStep::Configure);
    }
    #[test]
    fn done_goes_to_done() {
        let r = InstallResult::Done { outcome: serde_json::Value::Null, verify: serde_json::Value::Null, pin: serde_json::Value::Null, injection_findings: vec![] };
        assert_eq!(next_step(&r), InstallStep::Done);
    }
}
```

- [ ] **Step 3: Run → FAIL** `cargo test -p aleph-panel --lib components::extensions::install_flow::tests`

- [ ] **Step 4: Implement `install_flow.rs`** — reducer + state machine component

```rust
use leptos::prelude::*;
use serde_json::Value;

use crate::api::extensions::{ExtensionsApi, InstallResult};
use crate::context::DashboardState;
use crate::views::extensions::StoreState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallStep { Hidden, Trust, Configure, Installing, Done, Failed }

#[must_use]
pub fn next_step(result: &InstallResult) -> InstallStep {
    match result {
        InstallResult::NeedsAck { .. } => InstallStep::Trust,
        InstallResult::Missing { .. } => InstallStep::Configure,
        InstallResult::Done { .. } => InstallStep::Done,
    }
}

/// Call install with current values+ack and route to the next step.
fn drive_install(state: DashboardState, store: StoreState, id: String, ack: bool) {
    store.installing.set(true);
    store.install_error.set(None);
    let values = Value::Object(store.config_values.get_untracked());
    spawn_local(async move {
        match ExtensionsApi::install(&state, id, values, ack).await {
            Ok(result) => {
                if let InstallResult::NeedsAck { disclosure, .. } = &result {
                    store.disclosure.set(Some(disclosure.clone()));
                }
                store.install_step.set(next_step(&result));
                store.installing.set(false);
            }
            Err(e) => { store.install_error.set(Some(e)); store.install_step.set(InstallStep::Failed); store.installing.set(false); }
        }
    });
}

#[component]
#[must_use]
pub fn InstallFlow() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let store = expect_context::<StoreState>();

    // start_install set a target → fire the first install probe (no values, no ack).
    Effect::new(move || {
        if let Some(entry) = store.install_target.get() {
            store.install_target.set(None); // consume
            store.install_step.set(InstallStep::Installing);
            drive_install(state, store, entry.id.clone(), false);
        }
    });

    let close = move |_| { store.install_step.set(InstallStep::Hidden); store.disclosure.set(None); };

    view! {
        // Trust modal (Task 8 trust_modal.rs) shown on Trust step; on "Continue"/ack → re-install with ack=true.
        <crate::components::extensions::trust_modal::TrustModal
            on_continue=move |_| {
                if let Some(e) = store.selected.get().or(None) { /* selected may differ; prefer the in-flight id */ }
                // use the disclosure-bound id: keep the last target id on the store (see note)
            }
            on_cancel=close.clone()
        />
        // Configure modal: JsonSchemaForm over fields_from(config_schema, disclosure.secrets, missing).
        // Installing/Done/Failed: lightweight status overlays + toast; on Done → refresh catalog+installed.
    }
}
```
> **Implementer-verify (the wiring seam):** the in-flight extension id must persist across steps. Add `pub install_id: RwSignal<Option<String>>` to `StoreState`, set it in `start_install`, and use it in `drive_install` calls from the Trust "Continue" (ack=true) and Configure "Install" (values+ack=true) handlers — do NOT rely on `selected` (the user may close the drawer mid-flow). On `InstallStep::Done`, re-run the catalog loader (Task 4 `load_catalog`) and, if the installed view is mounted, its loader (Task 9) so the card flips to "Installed". The Configure modal builds fields with `crate::components::json_schema_form::fields_from(entry.config_schema.as_ref(), &disclosure.secrets, &missing)` — keep the `missing` list from the `Missing` branch.

- [ ] **Step 5: `trust_modal.rs`** — disclosure modal (R6 centered modal). Props: `on_continue: Callback<()>`, `on_cancel: Callback<()>`; reads `store.install_step == Trust` and `store.disclosure`. Renders: mono eyebrow + serif title; verdict banner (`risk_banner_class`); kv rows (publisher/tier, version `+ pinned`, integrity `sha256 ✓` when present, secrets count); a `<details>` "Command that will run" with the mono `command_display` (full, untruncated) + copy; an **ack checkbox** (amber wash) shown only when `disclosure.ack_required`; footer Cancel + Continue (Continue disabled until ack checked when required). On Continue → `on_continue` (flow re-installs with ack=true).

```rust
// key fragments — full markup mirrors R6 centered-modal scaffold
let ack = RwSignal::new(false);
// verdict:
view!{ <div class=format!("p-3 rounded border text-sm {}", crate::views::extensions::model::risk_banner_class(&d.risk))><strong>{d.one_line.clone()}</strong></div> }
// command disclose:
{d.command_display.clone().map(|cmd| view!{ <details class="mt-2"><summary class="text-xs text-text-secondary cursor-pointer">{t!(i18n, extensions.command_label)}</summary><pre class="font-mono text-xs bg-surface-sunken p-2 rounded mt-1 whitespace-pre-wrap break-all">{cmd}</pre></details> })}
// ack (only when required):
{d.ack_required.then(|| view!{ <label class="flex items-start gap-2 p-2 bg-warning-subtle rounded text-xs"><input type="checkbox" prop:checked=move||ack.get() on:change=move|ev| ack.set(event_target_checked(&ev)) />{t!(i18n, extensions.ack)}</label> })}
// Continue disabled until ack when required:
<button disabled=move || d.ack_required && !ack.get() on:click=move|_| on_continue.run(()) ... >{t!(i18n, extensions.continue_install)}</button>
```

- [ ] **Step 6: Wire Install buttons.** Card (Task 4): the Install button `on:click` → `store.start_install(entry.clone())` (instead of just selecting). Drawer (Task 6): footer Install `on:click` → `store.start_install(entry.clone())` then close the drawer (`store.selected.set(None)`). Mount `<crate::components::extensions::install_flow::InstallFlow/>` in `ExtensionsView`.

- [ ] **Step 7: Leave-guard.** In `ExtensionsView`'s "← Back to chat" handler, if `store.installing.get()` → `web_sys::window().confirm_with_message("Install in progress; leaving cancels it. Continue?")` (gate navigation on the result). Keep minimal; note the manual-router limits full interception.

- [ ] **Step 8: i18n.** Add to both locales under `extensions`: `"command_label":"Command that will run"`, `"ack":"I understand this runs third-party code on my computer."`, `"continue_install":"Continue"`, `"cancel":"Cancel"`, `"configure_title":"Configure"`, `"install_and_verify":"Install & verify"`, `"installing":"Installing…"`, `"install_done":"Installed ✓"`, `"install_failed":"Install failed"`, `"leave_confirm":"Install in progress; leaving cancels it. Continue?"` (zh translations). The `leave_confirm` string is read via `t_string!` for the `confirm()` call.

- [ ] **Step 9: Run pure tests → PASS** `cargo test -p aleph-panel --lib components::extensions::install_flow::tests`; **compile gate** `cargo check -p aleph-panel --target wasm32-unknown-unknown` — clean.
- [ ] **Step 10: Manual verify:** Install on a no-config Official MCP → installs directly (no ack/config) → card flips to Installed. Install a Community stdio MCP → trust modal (red verdict, command, ack required) → Continue (after ack) → if secrets required, config wizard (masked secret field) → Install & verify → success → Installed. Cancel at any step closes cleanly.
- [ ] **Step 11: Commit** `feat(panel): trust modal + install state machine + config wizard wiring + leave-guard`

---

### Task 9: Installed view (slide-in) — toggle / remove via `local:` ids

**Files:** Create `interfaces/webchat/src/views/extensions/installed.rs`; add an "Installed" button to the store topbar (in `ExtensionsView` or `browse.rs`) that sets `store.show_installed = true`; mount `<InstalledPanel/>` in `ExtensionsView`; register `pub mod installed;`. Add installed i18n keys to both locales.

**Interfaces:**
- Consumes: `ExtensionsApi::{installed, toggle, uninstall}` (lifecycle ops use the reconciled list's `local:` ids), `model::{kind_badge_class, kind_label_key, trust_dot_class, trust_label_key}`, `ConfirmButton` (R5).
- Produces: `#[component] InstalledPanel()` — right slide-in listing all reconciled installed extensions with enable/disable toggle, Remove (two-step confirm), update badge, and a "manual · not in catalog" tag for unmatched items.

- [ ] **Step 1: `installed.rs`** — slide-in panel + rows

```rust
use leptos::prelude::*;

use crate::api::extensions::{ExtensionEntry, ExtensionsApi};
use crate::components::ui::ConfirmButton;
use crate::context::DashboardState;
use crate::i18n::{t, use_i18n};
use crate::views::extensions::model::{kind_badge_class, kind_label_key, trust_dot_class, trust_label_key};
use crate::views::extensions::StoreState;

fn load_installed(state: DashboardState, items: RwSignal<Vec<ExtensionEntry>>, loading: RwSignal<bool>, error: RwSignal<Option<String>>) {
    loading.set(true);
    error.set(None);
    spawn_local(async move {
        match ExtensionsApi::installed(&state).await {
            Ok(list) => { items.set(list); loading.set(false); }
            Err(e) => { error.set(Some(format!("Failed to load installed: {e}"))); loading.set(false); }
        }
    });
}

#[component]
#[must_use]
pub fn InstalledPanel() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let store = expect_context::<StoreState>();
    let i18n = use_i18n();
    let items = RwSignal::new(Vec::<ExtensionEntry>::new());
    let loading = RwSignal::new(false);
    let error = RwSignal::new(Option::<String>::None);

    Effect::new(move || { if store.show_installed.get() && state.is_connected.get() { load_installed(state, items, loading, error); } });

    let close = move |_| store.show_installed.set(false);

    view! {
        <Show when=move || store.show_installed.get()>
            <div class="fixed inset-0 z-40 flex justify-end">
                <div class="aleph-scrim absolute inset-0 bg-black/30" on:click=close></div>
                <aside class="glass relative w-[480px] max-w-[94vw] h-full bg-surface-overlay/85 border-l border-border shadow-xl flex flex-col">
                    <header class="px-4 py-3 border-b border-border flex items-center justify-between">
                        <h2 class="font-serif text-lg text-text-primary">{t!(i18n, extensions.installed_title)}</h2>
                        <button class="text-text-tertiary hover:text-text-primary" on:click=close>"✕"</button>
                    </header>
                    <div class="flex-1 overflow-y-auto p-4 space-y-2">
                        {move || error.get().map(|e| view!{ <div class="p-3 bg-danger-subtle border border-border rounded text-danger text-sm">{e}</div> })}
                        {move || if loading.get() {
                            view!{ <div class="flex items-center justify-center py-12"><div class="animate-spin rounded-full h-8 w-8 border-b-2 border-primary"></div></div> }.into_any()
                        } else if items.get().is_empty() {
                            view!{ <div class="text-center py-12 border border-dashed border-border rounded-xl"><p class="text-text-secondary">{t!(i18n, extensions.none_installed)}</p></div> }.into_any()
                        } else {
                            view!{ <For each=move || items.get() key=|e: &ExtensionEntry| e.id.clone()
                                children=move |e| view!{ <InstalledRow entry=e items=items error=error /> } /> }.into_any()
                        }}
                    </div>
                </aside>
            </div>
        </Show>
    }
}

#[component]
fn InstalledRow(entry: ExtensionEntry, items: RwSignal<Vec<ExtensionEntry>>, error: RwSignal<Option<String>>) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();
    let enabled = RwSignal::new(entry.enabled);
    let toggling = RwSignal::new(false);
    let confirming = RwSignal::new(false);
    let id = entry.id.clone();
    let id_for_toggle = id.clone();
    let id_for_remove = id.clone();
    let badge_cls = format!("px-1.5 py-0.5 rounded text-[10px] font-mono font-bold uppercase {}", kind_badge_class(&entry.kind));
    // Heuristic: an unmatched/manual item is Unverified + not in any source catalog.
    let manual = entry.trust_tier == "unverified";

    let on_toggle = move |ev: leptos::ev::Event| {
        let new_val = event_target_checked(&ev);
        enabled.set(new_val); toggling.set(true);
        let id = id_for_toggle.clone();
        spawn_local(async move {
            match ExtensionsApi::toggle(&state, id, new_val).await {
                Ok(()) => toggling.set(false),
                Err(e) => { error.set(Some(format!("Toggle failed: {e}"))); enabled.set(!new_val); toggling.set(false); }
            }
        });
    };
    let on_remove = move || {
        let id = id_for_remove.clone();
        spawn_local(async move {
            match ExtensionsApi::uninstall(&state, id).await {
                Ok(()) => load_installed(state, items, RwSignal::new(false), error),
                Err(e) => error.set(Some(format!("Remove failed: {e}"))),
            }
        });
    };

    view! {
        <div class="p-3 bg-surface-raised border border-border rounded-xl flex items-center gap-3">
            <div class="w-10 h-10 rounded-lg bg-primary-subtle flex items-center justify-center flex-shrink-0">{entry.name.chars().next().map(|c| c.to_string()).unwrap_or_default()}</div>
            <div class="min-w-0 flex-1">
                <div class="flex items-center gap-2"><span class="text-text-primary truncate">{entry.name.clone()}</span><span class=badge_cls>{t!(i18n, [kind_label_key(&entry.kind)])}</span></div>
                <p class="text-xs text-text-tertiary truncate">
                    {entry.version.clone().map(|v| format!("v{v}")).unwrap_or_default()}
                    {manual.then(|| view!{ <span class="ml-2 px-1.5 py-0.5 border border-dashed border-border rounded font-mono text-[10px]">{t!(i18n, extensions.manual_tag)}</span> })}
                    {entry.update_available.then(|| view!{ <span class="ml-2 px-1.5 py-0.5 bg-warning-subtle text-warning rounded text-[10px]">{t!(i18n, extensions.update_available)}</span> })}
                </p>
            </div>
            <label class="relative inline-flex items-center cursor-pointer">
                <input type="checkbox" class="sr-only peer" prop:checked=move || enabled.get() on:change=on_toggle disabled=move || toggling.get() />
                <div class="w-11 h-6 bg-surface-sunken rounded-full peer peer-checked:bg-primary peer-checked:after:translate-x-full after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:rounded-full after:h-5 after:w-5 after:transition-all"></div>
            </label>
            {move || if confirming.get() {
                view!{ <ConfirmButton confirming=confirming on_confirm=on_remove.clone() label=None width_class="".to_string() size_class=None stop_propagation=false /> }.into_any()
            } else {
                view!{ <button class="px-2 py-1 text-xs text-danger hover:bg-danger-subtle rounded" on:click=move |_| confirming.set(true)>{t!(i18n, extensions.remove)}</button> }.into_any()
            }}
        </div>
    }
}
```
> Implementer-verify: `ConfirmButton`'s exact prop signature (R5 — `components/ui/confirm_button.rs`); adapt the call. The installed list's ids are already `local:{kind}:{backend}` (the only valid ids for `toggle`/`uninstall`) — do NOT substitute catalog ids. The `manual` heuristic (Unverified tier) is a reasonable v1 proxy for "not in catalog"; if the backend later flags reconciliation source, prefer that.

- [ ] **Step 2: Topbar Installed button.** In `ExtensionsView` header (next to the title) add `<button class="px-3 py-1.5 bg-surface-sunken text-text-secondary rounded-lg text-sm hover:text-text-primary" on:click=move |_| store.show_installed.set(true)>{t!(i18n, extensions.installed)} </button>`. Mount `<crate::views::extensions::installed::InstalledPanel/>` in `ExtensionsView`. Add `pub mod installed;`.

- [ ] **Step 3: i18n.** Add to both locales under `extensions`: `"installed_title":"Installed"`, `"none_installed":"No extensions installed yet."`, `"manual_tag":"manual · not in catalog"`, `"update_available":"Update available"`, `"remove":"Remove"` (zh: 已安装 / 尚未安装任何扩展 / 手动 · 不在目录中 / 有可用更新 / 移除).

- [ ] **Step 4: Compile gate** `cargo check -p aleph-panel --target wasm32-unknown-unknown` — clean.
- [ ] **Step 5: Manual verify:** the Installed button slides in a panel listing reconciled installs (incl. pre-store/manual MCP tagged "manual · not in catalog"); the enable/disable toggle calls `extensions.toggle` (optimistic, reverts on error); Remove → two-step confirm → `extensions.uninstall` → row disappears; after a fresh install (Task 8) the new item appears here.
- [ ] **Step 6: Final whole-phase build + smoke.** Run `cargo check -p aleph-panel --target wasm32-unknown-unknown` (or `trunk build`) clean; manual end-to-end against the mockup (`docs/superpowers/specs/2026-06-19-extensions-store-mockup.html`): browse-by-category, search/filter, drawer, trust modal, config wizard, install→verify, installed toggle/remove, back-to-chat. Note any parity gaps in the ledger as Minor follow-ups (don't silently drop).
- [ ] **Step 7: Commit** `feat(panel): installed view — reconciled list, toggle/remove via local: ids`

---

## Self-review (P3)

**Spec coverage (§12 + Decisions):**
- Top-level `PanelMode::Extensions`, full-screen takeover, grouped with Teams, "← Back to chat" → T3 ✓ (Decision #2).
- Functional-category browse PRIMARY + kind/trust SECONDARY badge/filter → T2 (model) + T4 (card badge) + T5 (chips/segs) ✓ (Decision #13).
- Featured strip + per-category shelves → T5 ✓ (featured is a deterministic Official/Verified stand-in until P4 curation — flagged, not fabricated).
- Card (icon/name/author/blurb/kind badge/trust dot/install) → T4 ✓.
- Detail drawer + "What it can reach" risk banners + secrets → T6 ✓ (§12.2, §11 risk classes).
- Pre-install trust disclosure modal (verdict + command + secrets + version + sha256 + ack) → T8 ✓ (§11; full untruncated command per injection-hardening).
- Config wizard `json_schema_form.rs` (masked secrets) → T7 ✓ (net-new, §12.4).
- Installed view (reconciled, manual tag, toggle/remove, update badge) → T9 ✓ (§7, §12.5).
- Install UX flow (install → trust → wizard → progress → verify) + leave-confirm → T8 ✓ (§10, Decision #5).
- i18n en/zh → folded into every task (compile requirement) ✓.

**Deviations (flagged, not silent):**
- **i18n in P3, not P5.** The store's own keys MUST land in P3 (panel won't compile with dangling `t!` keys). P5's i18n scope becomes the migration pieces (demoted panels, removed ClawHub). Documented in Global Constraints.
- **No new trust-color tokens.** Trust tiers map to existing tokens (theme-aware, avoids the light/dark/system mirror verbatim-copy invariant). The "gallery" identity comes from the Fraunces serif + paper-toned surfaces, per the i18n/design dossier's default recommendation — not the mockup's bespoke teal.
- **`sources.add/remove` dropped from the UI** (not implemented backend-side; out of MVP closed loop). `sources.list`/`refresh` available if a "Refresh catalog" affordance is wanted; no add/remove UI invented.
- **Detail re-fetch-by-id avoided** (no such RPC; `CatalogParams` lacks `id`). The drawer reuses the held `ExtensionEntry`; permissions via `extensions.disclosure`.
- **Featured = deterministic stand-in** for Store-Agent editorial picks (P4 replaces `featured_picks`).

**Placeholder scan:** pure tasks (T1, T2, T7-builder, T8-reducer) carry complete code + tests. View tasks carry complete component code for the novel parts + exact reusable scaffolds (R6), exact token classes, exact RPC calls, and explicit "implementer-verify" notes anchored to R5 primitive signatures and the two macro-form/prop-signature unknowns (`t!` dynamic key; `SelectInput`/`ConfirmButton` props) — consistent with how P0–P2 were authored and the INDEX P3 test strategy ("component logic unit tests + manual run; visual parity").

**Type consistency:** `ExtensionEntry`/`DisclosurePayload`/`SecretDisclosure`/`InjectionFinding`/`InstallResult` (T1) are the single DTO contract consumed unchanged by T2–T9; `Filters`/`CATEGORIES`/class-maps (T2) consumed by T4/T5/T6/T9; `FieldSpec`/`fields_from` (T7) consumed by T8's Configure modal; `InstallStep`/`next_step` (T8) own the flow; `StoreState` (T3, extended in T8) is the shared context across all views. The `local:` (lifecycle) vs catalog (install/disclosure) id namespaces are kept distinct (T9 uses installed-list ids; T6/T8 use catalog ids).

**Execution focus for reviewers:** T7 `fields_from` and T8 `next_step`/`drive_install` are the load-bearing logic — verify the field union (disclosure.secrets ∪ schema, sensitivity from disclosure) and that the in-flight `install_id` (not `selected`) drives the ack/configure re-install calls. T8 must persist the id across steps and refresh catalog+installed on Done. Every view must gate the first fetch on `is_connected` and reset busy signals on both success and error.
