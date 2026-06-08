# Route Usage-Based Panel Wiring Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the Panel route page read and write `load_balance` + per-provider `rate_limits`, fixing the silent-data-loss clobber and completing the deferred usage-based routing form.

**Architecture:** Pure Panel change. The gateway `route_config.{get,update}` RPC already serves and accepts both fields — only the WASM client structs and the Leptos form are blind to them. We add the fields to `RouteConfigView`/`RouteConfigUpdate` (backward-compatible `#[serde(default)]`), render a strategy `<select>` and a per-provider rpm/tpm editor, and include both in the save payload. Backend is untouched.

**Tech Stack:** Rust, Leptos 0.8 (CSR/WASM), leptos_i18n 0.6 (compile-time locale tables from `locales/{en,zh}.json`), serde.

**Standing constraint (overrides TDD "run" steps):** Per project rule, **do not run `cargo check` / `just wasm` / host build** in this session. The panel is wasm32-target and cannot be blind-checked — this is the exact reason this form was deferred originally. Tests are authored as the post-merge regression guard; correctness in-session comes from mirroring the proven idioms already in `route.rs`. Each "run" step below notes this explicitly.

**Worktree:** `/Volumes/TBU4/Workspace/Aleph-wt-route-panel` (branch `feat/route-usage-panel-wiring`). All `git`/file paths below are relative to this worktree root.

---

## File Structure

| File | Responsibility | Change |
|------|----------------|--------|
| `interfaces/webchat/src/api/settings.rs` | RPC DTOs for `route_config` | Add `RateLimit` struct; add `load_balance` + `rate_limits` to `RouteConfigView` & `RouteConfigUpdate`; serde tests |
| `interfaces/webchat/locales/en.json` | English i18n table | Add `settings.route.*` keys for strategy + rate limits |
| `interfaces/webchat/locales/zh.json` | Chinese i18n table (must match en key-set exactly) | Same keys, translated |
| `interfaces/webchat/src/views/settings/route.rs` | Route settings Leptos view | New signals + load + save round-trip; strategy `<select>`; `RateLimitEditor` component |

`RateLimit` auto-re-exports via the existing `pub use settings::*;` at `interfaces/webchat/src/api.rs:62`, so `use crate::api::RateLimit;` resolves with no extra wiring.

---

## Task 1: API DTOs — add `RateLimit` + new fields + serde guard tests

**Files:**
- Modify: `interfaces/webchat/src/api/settings.rs` (top import + structs at lines 156–195)
- Test: same file, new `#[cfg(test)] mod route_serde_tests`

- [ ] **Step 1: Add `BTreeMap` import**

At the top of `interfaces/webchat/src/api/settings.rs`, the current imports are:

```rust
use crate::context::DashboardState;
use serde::{Deserialize, Serialize};
use serde_json::Value;
```

Change to:

```rust
use crate::context::DashboardState;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
```

- [ ] **Step 2: Add the `RateLimit` mirror struct**

Insert immediately **before** `pub struct RouteConfigView` (currently line 167, just after the `RouteProviderInfo` struct closes at line 165):

```rust
/// Per-provider soft rate ceiling, mirroring the backend `ProviderRateLimit`
/// (`[route.rate_limits.<provider>]`). `skip_serializing_if` matches the wire
/// bytes exactly: an omitted dimension means "unbounded on that axis", so the
/// `usage_based` strategy treats it as infinite headroom.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RateLimit {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rpm: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tpm: Option<u32>,
}
```

- [ ] **Step 3: Add fields to `RouteConfigView`**

In `pub struct RouteConfigView`, after the `providers` field (line 182), add the two fields so the struct ends:

```rust
    #[serde(default)]
    pub providers: Vec<RouteProviderInfo>,
    /// Active load-balancing strategy: "ordered" | "round_robin" | "least_busy"
    /// | "latency_aware" | "usage_based". `None` from an older daemon → treated
    /// as "ordered" by the view.
    #[serde(default)]
    pub load_balance: Option<String>,
    /// Per-provider rpm/tpm ceilings keyed by provider name. Empty when unset.
    #[serde(default)]
    pub rate_limits: BTreeMap<String, RateLimit>,
}
```

- [ ] **Step 4: Add fields to `RouteConfigUpdate`**

In `pub struct RouteConfigUpdate`, after the `cloud_provider` field (line 194), add:

```rust
    #[serde(default)]
    pub cloud_provider: Option<String>,
    /// Chosen load-balancing strategy (same key set as the view). Sent on every
    /// save so the backend full-replace does not reset it to `Ordered`.
    #[serde(default)]
    pub load_balance: Option<String>,
    /// Per-provider rpm/tpm ceilings. Sent on every save so the backend
    /// full-replace does not wipe configured limits.
    #[serde(default)]
    pub rate_limits: BTreeMap<String, RateLimit>,
}
```

- [ ] **Step 5: Add serde regression tests**

Append to the end of `interfaces/webchat/src/api/settings.rs`:

```rust
#[cfg(test)]
mod route_serde_tests {
    use super::*;

    #[test]
    fn rate_limit_omits_none_dims_on_wire() {
        let rl = RateLimit { rpm: Some(60), tpm: None };
        assert_eq!(serde_json::to_value(&rl).unwrap(), serde_json::json!({ "rpm": 60 }));
    }

    #[test]
    fn update_round_trips_strategy_and_limits() {
        let mut rate_limits = BTreeMap::new();
        rate_limits.insert("anthropic".to_string(), RateLimit { rpm: Some(60), tpm: Some(90_000) });
        let u = RouteConfigUpdate {
            mode: "auto".into(),
            allow_cloud_escalation: false,
            local_provider: None,
            cloud_provider: None,
            load_balance: Some("usage_based".into()),
            rate_limits,
        };
        let j = serde_json::to_value(&u).unwrap();
        assert_eq!(j["load_balance"], "usage_based");
        assert_eq!(j["rate_limits"]["anthropic"]["rpm"], 60);
        assert_eq!(j["rate_limits"]["anthropic"]["tpm"], 90_000);
    }

    #[test]
    fn view_tolerates_absent_new_fields() {
        // An older daemon response without the new keys must still parse — this
        // is the backward-compatibility guarantee.
        let v: RouteConfigView =
            serde_json::from_value(serde_json::json!({ "mode": "auto" })).unwrap();
        assert_eq!(v.mode, "auto");
        assert!(v.load_balance.is_none());
        assert!(v.rate_limits.is_empty());
    }
}
```

- [ ] **Step 6: (Deferred run) Note the test command**

Run (POST-MERGE ONLY — **not** executed this session per the standing constraint):

```bash
cargo test -p aleph-panel route_serde_tests
```

Expected when a human runs it post-merge: 3 passed. In-session: skip; the tests document the contract.

- [ ] **Step 7: Commit**

```bash
cd /Volumes/TBU4/Workspace/Aleph-wt-route-panel
git add interfaces/webchat/src/api/settings.rs
git commit -m "panel(api): add load_balance + rate_limits to route_config DTOs"
```

---

## Task 2: i18n keys — strategy + rate-limit labels (en + zh in lockstep)

**Files:**
- Modify: `interfaces/webchat/locales/en.json` (`settings.route` object)
- Modify: `interfaces/webchat/locales/zh.json` (`settings.route` object)

> leptos_i18n generates a compile-time table and requires **identical key sets** across locales. Both files must gain the same keys in this one commit (the `runtime_deps` i18n-sweep hazard from a prior session — keep them in lockstep).

- [ ] **Step 1: Add keys to `en.json`**

In `interfaces/webchat/locales/en.json`, inside the `settings.route` object, after the existing `"no_providers"` entry, add (insert a comma after the current last entry):

```json
    "load_balance": "Load balancing",
    "load_balance_desc": "How requests spread across providers within the same tier. Ordered keeps your configured order; Usage-based spreads by remaining rpm/tpm headroom and de-prioritises saturated providers.",
    "lb_ordered": "Ordered (configured order)",
    "lb_round_robin": "Round-robin",
    "lb_least_busy": "Least busy",
    "lb_latency_aware": "Latency-aware",
    "lb_usage_based": "Usage-based (rpm/tpm headroom)",
    "rate_limits": "Rate limits",
    "rate_limits_desc": "Optional per-provider soft ceilings (LiteLLM-style). Used by the Usage-based strategy. Leave blank for unlimited on that axis.",
    "rpm": "Requests / min",
    "tpm": "Tokens / min",
    "unlimited": "Unlimited"
```

- [ ] **Step 2: Add the same keys to `zh.json`**

In `interfaces/webchat/locales/zh.json`, inside the `settings.route` object, after the existing `"no_providers"` entry, add:

```json
    "load_balance": "负载均衡",
    "load_balance_desc": "请求在同一层级的提供商之间如何分配。Ordered 保持你配置的顺序；Usage-based 按剩余 rpm/tpm 余量分配并降权已饱和的提供商。",
    "lb_ordered": "顺序（配置顺序）",
    "lb_round_robin": "轮询",
    "lb_least_busy": "最少繁忙",
    "lb_latency_aware": "延迟感知",
    "lb_usage_based": "用量感知（rpm/tpm 余量）",
    "rate_limits": "速率上限",
    "rate_limits_desc": "可选的每提供商软上限（LiteLLM 风格）。Usage-based 策略会使用它。留空表示该维度不限。",
    "rpm": "请求/分钟",
    "tpm": "Token/分钟",
    "unlimited": "不限"
```

- [ ] **Step 3: Validate JSON + key parity**

Run (pure host tooling — allowed, no cargo/wasm):

```bash
cd /Volumes/TBU4/Workspace/Aleph-wt-route-panel
python3 -c "import json; a=json.load(open('interfaces/webchat/locales/en.json'))['settings']['route']; b=json.load(open('interfaces/webchat/locales/zh.json'))['settings']['route']; assert set(a)==set(b), set(a)^set(b); print('key parity OK', len(a), 'keys')"
```

Expected: `key parity OK <N> keys` (no assertion error). If it prints a symmetric-difference set, a key is missing from one file — fix it.

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/locales/en.json interfaces/webchat/locales/zh.json
git commit -m "panel(i18n): add route load_balance + rate_limit keys (en/zh)"
```

---

## Task 3: route.rs — signals, load, and save round-trip (kills the clobber)

**Files:**
- Modify: `interfaces/webchat/src/views/settings/route.rs` (import line 9; signals ~24–33; load ~38–46; save closure ~62–68)

- [ ] **Step 1: Extend the api import + add `BTreeMap`**

Change line 9 from:

```rust
use crate::api::{RouteConfigApi, RouteConfigUpdate, RouteProviderInfo};
```

to:

```rust
use crate::api::{RateLimit, RouteConfigApi, RouteConfigUpdate, RouteProviderInfo};
use std::collections::BTreeMap;
```

- [ ] **Step 2: Add the two new signals**

After the `providers` signal (line 29), add:

```rust
    let providers = RwSignal::new(Vec::<RouteProviderInfo>::new());
    let load_balance = RwSignal::new(String::from("ordered"));
    let rate_limits = RwSignal::new(BTreeMap::<String, RateLimit>::new());
```

(The first line is the existing one shown for anchor context; add only the two new lines.)

- [ ] **Step 3: Populate them on load**

In the `Ok(view)` arm of the load block (after `providers.set(view.providers);`, line 44), add:

```rust
                    providers.set(view.providers);
                    load_balance.set(view.load_balance.unwrap_or_else(|| "ordered".into()));
                    rate_limits.set(view.rate_limits);
                    loading.set(false);
```

(First and last lines are existing anchors; add the two middle lines.)

- [ ] **Step 4: Include both in the save payload**

In the `save` closure, the `RouteConfigUpdate` literal (lines 63–68) currently is:

```rust
            let update = RouteConfigUpdate {
                mode: mode.get(),
                allow_cloud_escalation: allow_escalation.get(),
                local_provider: to_pin(local_provider.get()),
                cloud_provider: to_pin(cloud_provider.get()),
            };
```

Change to:

```rust
            let update = RouteConfigUpdate {
                mode: mode.get(),
                allow_cloud_escalation: allow_escalation.get(),
                local_provider: to_pin(local_provider.get()),
                cloud_provider: to_pin(cloud_provider.get()),
                load_balance: Some(load_balance.get()),
                rate_limits: rate_limits.get(),
            };
```

- [ ] **Step 5: Add the `parse_limit` helper**

At the bottom of `route.rs` (after the `ProviderTierSelect` component, end of file), add the free function used by the editor in Task 5:

```rust
/// Parse a number-input string into an optional ceiling. Empty / non-numeric →
/// `None` (that dimension is unbounded). Mirrors the backend's "omitted = no
/// limit" contract.
fn parse_limit(raw: &str) -> Option<u32> {
    let t = raw.trim();
    if t.is_empty() {
        None
    } else {
        t.parse::<u32>().ok()
    }
}
```

- [ ] **Step 6: (Deferred run) Note the build command**

Build (POST-MERGE ONLY — **not** executed this session per the standing constraint):

```bash
just wasm    # rebuilds interfaces/webchat/dist/*
```

In-session: skip. Correctness comes from the structural match to existing signal/load/save idioms in this same file.

- [ ] **Step 7: Commit**

```bash
git add interfaces/webchat/src/views/settings/route.rs
git commit -m "panel(route): round-trip load_balance + rate_limits on save (fix clobber)"
```

---

## Task 4: route.rs — load-balance strategy `<select>`

**Files:**
- Modify: `interfaces/webchat/src/views/settings/route.rs` (const near line 17; view block — insert after the mode-selector `</div>` at line 131)

- [ ] **Step 1: Add the strategy key list**

After `const MODE_KEYS` (line 17), add:

```rust
/// Load-balancing strategy keys, matched 1:1 to the backend
/// `LoadBalanceStrategy` serde names.
const LB_KEYS: &[&str] = &[
    "ordered",
    "round_robin",
    "least_busy",
    "latency_aware",
    "usage_based",
];
```

- [ ] **Step 2: Render the strategy select**

In the `view!` block, immediately **after** the mode-selector-cards `<div>` closes (the `</div>` at line 131, before the `// Cloud-escalation toggle` `<Show>` at line 134), insert:

```rust
                    // Load-balancing strategy — how same-tier providers are
                    // ordered within the active route. Default "ordered" is a
                    // no-op (configured order).
                    <div class="bg-surface-raised rounded-lg border border-border p-4">
                        <label class="block font-semibold text-text-primary mb-1">
                            {t!(i18n, settings.route.load_balance)}
                        </label>
                        <p class="text-sm text-text-secondary mb-2">
                            {t!(i18n, settings.route.load_balance_desc)}
                        </p>
                        <select
                            class="w-full bg-surface border border-border rounded-lg px-3 py-2 text-sm text-text-primary"
                            prop:value=move || load_balance.get()
                            on:change=move |ev| {
                                load_balance.set(event_target_value(&ev));
                                saved.set(false);
                            }
                        >
                            {LB_KEYS.iter().map(|key| {
                                let key = *key;
                                let label = move || match key {
                                    "ordered" => t_string!(i18n, settings.route.lb_ordered),
                                    "round_robin" => t_string!(i18n, settings.route.lb_round_robin),
                                    "least_busy" => t_string!(i18n, settings.route.lb_least_busy),
                                    "latency_aware" => t_string!(i18n, settings.route.lb_latency_aware),
                                    _ => t_string!(i18n, settings.route.lb_usage_based),
                                };
                                view! { <option value=key>{label}</option> }
                            }).collect::<Vec<_>>()}
                        </select>
                    </div>
```

- [ ] **Step 3: (Deferred run)** Same as Task 3 Step 6 — `just wasm` is post-merge only; skip in-session.

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/views/settings/route.rs
git commit -m "panel(route): add load-balance strategy selector"
```

---

## Task 5: route.rs — per-provider rate-limit editor

**Files:**
- Modify: `interfaces/webchat/src/views/settings/route.rs` (view block — insert after the preferred-providers `<div>` block at line 199; new `RateLimitEditor` component after `ProviderTierSelect`)

- [ ] **Step 1: Render the editor in the view**

In the `view!` block, immediately **after** the preferred-providers `<div class="pt-2">…</div>` block closes (the `</div>` at line 199, before the outer `</div>` of the `space-y-6` container at line 200), insert:

```rust
                    // Per-provider rpm/tpm ceilings (used by Usage-based).
                    <RateLimitEditor
                        providers=providers
                        rate_limits=rate_limits
                        saved=saved
                    />
```

- [ ] **Step 2: Add the `RateLimitEditor` component**

At the bottom of `route.rs`, **after** the `ProviderTierSelect` component and **before** the `parse_limit` fn added in Task 3 (order within the file doesn't matter; place it after `ProviderTierSelect`), add:

```rust
/// Per-provider soft rate-limit editor. Iterates every configured provider and
/// exposes two optional number inputs (rpm / tpm). An empty field clears that
/// dimension; clearing both removes the provider's entry entirely, so the saved
/// `rate_limits` map stays minimal and byte-identical to a hand-written
/// `[route.rate_limits.*]` block.
#[component]
fn RateLimitEditor(
    providers: RwSignal<Vec<RouteProviderInfo>>,
    rate_limits: RwSignal<BTreeMap<String, RateLimit>>,
    saved: RwSignal<bool>,
) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <div class="pt-2">
            <h3 class="font-semibold text-text-primary mb-1">{t!(i18n, settings.route.rate_limits)}</h3>
            <p class="text-sm text-text-secondary mb-3">
                {t!(i18n, settings.route.rate_limits_desc)}
            </p>
            <div class="space-y-2">
                {move || providers.get().into_iter().map(|p| {
                    let name = p.name.clone();
                    let name_rpm = name.clone();
                    let name_tpm = name.clone();
                    let rpm_val = {
                        let name = name.clone();
                        move || rate_limits.get().get(&name).and_then(|r| r.rpm)
                            .map(|v| v.to_string()).unwrap_or_default()
                    };
                    let tpm_val = {
                        let name = name.clone();
                        move || rate_limits.get().get(&name).and_then(|r| r.tpm)
                            .map(|v| v.to_string()).unwrap_or_default()
                    };
                    view! {
                        <div class="flex items-center gap-3 bg-surface-raised rounded-lg border border-border p-3">
                            <span class="flex-1 text-sm text-text-primary truncate">{p.name.clone()}</span>
                            <input
                                type="number"
                                min="0"
                                class="w-28 bg-surface border border-border rounded px-2 py-1 text-sm text-text-primary"
                                title=move || t_string!(i18n, settings.route.rpm).to_string()
                                placeholder=move || t_string!(i18n, settings.route.unlimited).to_string()
                                prop:value=rpm_val
                                on:input=move |ev| {
                                    let v = parse_limit(&event_target_value(&ev));
                                    let key = name_rpm.clone();
                                    rate_limits.update(|m| {
                                        let e = m.entry(key.clone()).or_default();
                                        e.rpm = v;
                                        if e.rpm.is_none() && e.tpm.is_none() { m.remove(&key); }
                                    });
                                    saved.set(false);
                                }
                            />
                            <input
                                type="number"
                                min="0"
                                class="w-28 bg-surface border border-border rounded px-2 py-1 text-sm text-text-primary"
                                title=move || t_string!(i18n, settings.route.tpm).to_string()
                                placeholder=move || t_string!(i18n, settings.route.unlimited).to_string()
                                prop:value=tpm_val
                                on:input=move |ev| {
                                    let v = parse_limit(&event_target_value(&ev));
                                    let key = name_tpm.clone();
                                    rate_limits.update(|m| {
                                        let e = m.entry(key.clone()).or_default();
                                        e.tpm = v;
                                        if e.rpm.is_none() && e.tpm.is_none() { m.remove(&key); }
                                    });
                                    saved.set(false);
                                }
                            />
                        </div>
                    }
                }).collect::<Vec<_>>()}
                <Show when=move || providers.get().is_empty()>
                    <p class="text-xs text-text-tertiary">{t!(i18n, settings.route.no_providers)}</p>
                </Show>
            </div>
        </div>
    }
}
```

- [ ] **Step 3: (Deferred run)** Same as Task 3 Step 6 — `just wasm` is post-merge only; skip in-session.

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/views/settings/route.rs
git commit -m "panel(route): add per-provider rpm/tpm rate-limit editor"
```

---

## Task 6: Final review pass

- [ ] **Step 1: Diff the whole branch against main**

```bash
cd /Volumes/TBU4/Workspace/Aleph-wt-route-panel
git diff f858a23b0...HEAD --stat
```

Expected: exactly 4 files changed — `interfaces/webchat/src/api/settings.rs`, `interfaces/webchat/locales/en.json`, `interfaces/webchat/locales/zh.json`, `interfaces/webchat/src/views/settings/route.rs` (plus the two spec/plan docs from earlier commits).

- [ ] **Step 2: Confirm no backend file was touched**

```bash
git diff f858a23b0...HEAD --name-only | grep -E '^src/' && echo "BACKEND TOUCHED — STOP" || echo "backend clean (panel-only)"
```

Expected: `backend clean (panel-only)`.

- [ ] **Step 3: Confirm key parity once more**

```bash
python3 -c "import json; a=json.load(open('interfaces/webchat/locales/en.json'))['settings']['route']; b=json.load(open('interfaces/webchat/locales/zh.json'))['settings']['route']; assert set(a)==set(b); print('parity OK')"
```

Expected: `parity OK`.

---

## Manual Verification (post-merge, by a human running the daemon)

Not run in this session. The acceptance checks:

1. Set `load_balance = "usage_based"` + a `[route.rate_limits.<provider>]` block via TOML (or the natural-language config tool). Open Panel → Model Routing. **Expected:** the strategy select shows "Usage-based" and the provider's rpm/tpm fields show the configured values (today they are invisible).
2. Change only the mode, click **Apply**, reload the page. **Expected:** `rate_limits` and strategy **survive** (today they are silently wiped).
3. Edit an rpm/tpm value, **Apply**. **Expected:** value persists to the config TOML and hot-applies to the live `RouteHandle` (next prompt routes under the new ceiling).

---

## Self-Review Notes

- **Spec coverage:** api struct fields (Task 1) ✓; strategy select (Task 4) ✓; per-provider rpm/tpm editor (Task 5) ✓; i18n en+zh (Task 2) ✓; save round-trip fixes clobber (Task 3 Step 4) ✓; backend untouched (Task 6 Step 2) ✓; backward-compat via `#[serde(default)]` (Task 1) ✓.
- **Type consistency:** `RateLimit { rpm, tpm }` used identically in Task 1 (def), Task 3 (`BTreeMap<String, RateLimit>`), Task 5 (`.rpm`/`.tpm`, `or_default()` needs the `Default` derive added in Task 1 Step 2). `parse_limit` defined Task 3 Step 5, consumed Task 5 Step 2. `LB_KEYS` keys match the backend `lb_from_str` set (`ordered|round_robin|least_busy|latency_aware|usage_based`) verified at `src/gateway/handlers/route_config.rs:89-96`.
- **No placeholders:** every step shows full code or an exact command.
