# Route Usage-Based Panel Wiring — Design

**Date:** 2026-06-08
**Branch:** `feat/route-usage-panel-wiring` (worktree `Aleph-wt-route-panel`, off main `f858a23b0`)
**Type:** 错误修复 (bug fix) + 功能连线 (feature wiring)

## Context

A prior session (`feat/usage-aware-provider-routing`, merged into main as `110602449`) built
LiteLLM usage-based parity for **provider routing**: `LoadBalanceStrategy::UsageBased`,
per-provider `rate_limits` (`rpm`/`tpm`), a lock-free rolling `RateWindow`, and the failover
wiring that feeds real token usage back into the window and de-prioritises rate-saturated
providers. That work was committed **without `cargo check`** (per a standing constraint) and a
Panel `rate_limits` form was explicitly **deferred** because Leptos/WASM can't be blind-checked.

This session audits the merged code and closes that deferred wiring gap.

## Audit Finding (provider-side core is sound)

Static trace of the merged provider code found **no defect**, no change needed:

- `route_policy.rs::sort_by_metric` matches **all 5** strategy arms exhaustively (no wildcard) → compiles.
- Token feed is correct RAII: `LoadStats::begin()` per attempt; `record_tokens(input+output)` only on
  `Ok` with real `usage`; `Drop` decrements the in-flight counter on every exit path.
- Hot-reload is coherent: `ArcSwap<RateLimits>` + `AtomicU8` strategy; `lb_to_u8`/`u8_to_lb` round-trip;
  all test-covered in-file.
- Gateway `route_config.{get,update}` **already** serve and accept `load_balance` + `rate_limits`
  (`src/gateway/handlers/route_config.rs:136,139,183,206`).

## The Bug (HIGH — silent data loss)

The Panel client is blind to `load_balance` + `rate_limits`:

- `interfaces/webchat/src/api/settings.rs` — `RouteConfigView` (get response) and `RouteConfigUpdate`
  (update payload) **both omit** `load_balance` and `rate_limits`.
- The Panel `route.rs` "Apply" button therefore sends a payload **without** those fields.
- Backend `handle_update` rebuilds the **whole** `ModelRouteConfig` from the payload: absent
  `load_balance` → `Ordered` (`route_config.rs:184`), absent `rate_limits` → empty `BTreeMap`
  (serde default), then `cfg.route = new_route` + `save_incremental(&["route"])` **persists** it.

**Result:** any user who configured `usage_based` + `rate_limits` via TOML or the natural-language
config tool (R8), then later clicks "Apply" on the Panel route page (e.g. just to change the mode),
**silently wipes their entire `rate_limits` and resets the strategy to `Ordered`.** The prior
session's feature is destroyed by the panel's own save path.

Fixing this *is* completing the deferred form: round-tripping the fields removes the clobber, and
rendering an editor surfaces the feature in the UI.

## Design (Approach A — full form round-trip)

**Backend: zero changes.** The RPC contract is already complete.

**Scope: 4 panel files.**

### 1. `interfaces/webchat/src/api/settings.rs`
- New mirror struct (panel can't import `alephcore` types):
  ```rust
  #[derive(Debug, Clone, Default, Serialize, Deserialize)]
  pub struct RateLimit {
      #[serde(default, skip_serializing_if = "Option::is_none")]
      pub rpm: Option<u32>,
      #[serde(default, skip_serializing_if = "Option::is_none")]
      pub tpm: Option<u32>,
  }
  ```
  `skip_serializing_if` matches the backend `ProviderRateLimit` wire bytes exactly (omitted dim → unbounded).
- Add to **both** `RouteConfigView` and `RouteConfigUpdate`, each `#[serde(default)]` (backward-compatible —
  old daemons/payloads still parse):
  - `pub load_balance: Option<String>,`
  - `pub rate_limits: BTreeMap<String, RateLimit>,`

### 2. `interfaces/webchat/src/views/settings/route.rs`
- New signals: `load_balance: RwSignal<String>` (default `"ordered"`),
  `rate_limits: RwSignal<BTreeMap<String, RateLimit>>`.
- On load: populate both from the get response.
- **Strategy `<select>`** — options `ordered | round_robin | least_busy | latency_aware | usage_based`,
  labels/descriptions from i18n. Mirrors the existing mode-card render idiom; `on:change` sets the signal
  and clears the `saved` flag.
- **Per-provider rate-limit editor** — iterate the existing `providers` list; for each provider name render
  two optional number inputs (rpm, tpm) bound into the `rate_limits` map. Empty input → `None` for that
  dimension; both empty → entry omitted from the map. Reuses the `ProviderTierSelect` row layout idiom.
- **Save payload includes both fields** — this line kills the clobber.

### 3. `interfaces/webchat/locales/en.json` + `zh.json`
- New `settings.route.*` keys following the existing `mode_auto` / `allow_escalation` block:
  strategy section title + per-strategy label/desc (5), rate-limits section title + desc, `rpm` / `tpm`
  field labels + placeholder/help. Keep both locales in lockstep (the `runtime_deps` i18n-sweep hazard
  from a prior session — add to both files in the same commit).

## Data Flow

```
[Panel route.rs form]
   strategy select + per-provider rpm/tpm
        │ (RwSignals)
        ▼
RouteConfigUpdate { mode, .., load_balance, rate_limits }
        │ serde_json → route_config.update RPC   (UNCHANGED backend)
        ▼
handle_update → ModelRouteConfig → cfg.route → save_incremental(["route"])
        │
        ├─► RouteHandle::store()  → ArcSwap<RateLimits> + AtomicU8 strategy (hot-applies; next prompt routes anew)
        └─► persisted to config TOML
        ▲
        │ route_config.get  →  RouteConfigView { .., load_balance, rate_limits }
[Panel reloads form with persisted values]  ← round-trip closes the loop, no clobber
```

## Out of Scope (explicit)

- No provider-side code change (audit found it clean — no fabricated edits, 熵减原则).
- No new routing capability (budget governance, semantic cache, RouteLLM cost routing) — separate specs.
- No backend `handle_update`/`handle_get` change — contract already complete.

## Safety / Process

- All work in worktree `Aleph-wt-route-panel` (branch `feat/route-usage-panel-wiring`) — **never main**.
- Backward compatible: every new field is `#[serde(default)]`; empty `rate_limits` + `ordered` strategy is
  byte-identical to today's behaviour.
- Per standing constraint: **no `cargo check` / `wasm` build** after — Leptos/WASM can't be blind-checked
  anyway. Mitigation: mirror the proven idioms in `route.rs` / `settings_sidebar.rs` line-for-line.
- Manual worktree cleanup in a **separate** session (EnterWorktree-removal corrupts the shell — known hazard).

## Verification (manual, post-merge by a human running the daemon)

1. Set `load_balance = "usage_based"` + `rate_limits` via TOML; open Panel route page → fields show the
   configured values (currently they'd be invisible).
2. Change mode only, click Apply → reload → `rate_limits` + strategy **survive** (today they're wiped).
3. Edit rpm/tpm in the form, Apply → values persist to TOML and hot-apply to the live `RouteHandle`.
