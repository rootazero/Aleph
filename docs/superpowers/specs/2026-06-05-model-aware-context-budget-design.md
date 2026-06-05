# Model-Aware Context Compaction Budget

**Date:** 2026-06-05
**Status:** Approved (user 认可)
**Branch:** `feat/model-aware-context-budget`

## Problem

Aleph's mid-run context compaction triggers off a **flat, boot-time `token_budget`**
(`build_context_budget_config`, default 200k) that is identical regardless of which
model is actually running. A 200k-window model (Kimi K2) and a 1M-window model
(Claude 4.x beta) therefore compact at the *same absolute* point — too early for
the big model, and the flat default can be wrong for both. Model-aware compaction
timing is a baseline platform feature.

A per-model capability catalog already exists — `capabilities_for(model) ->
ModelCapabilities { context_window, max_output_tokens, .. }`
(`src/providers/model_catalog/capabilities.rs`) — but the compaction layer never
consults it. The catalog is also missing common families (Kimi/Moonshot, GLM,
Qwen).

## Reference designs

- **hermes-agent** (`agent/context_compressor.py`): runtime model lookup,
  `threshold = context_length * threshold_percent` (0.50). Percentage scales
  naturally with window size.
- **pi** (`packages/.../compaction.ts`): `shouldCompact = contextTokens >
  contextWindow - reserveTokens` with a fixed `reserveTokens = 16384` output
  margin; `contextWindow` from a model registry.
- **openclaw**: context window tracked for UI display only, not compaction.

Aleph adopts hermes's *percentage thresholds applied to a per-model budget* and
pi's *explicit output reserve*, but makes the reserve model-aware (derived from
the model's `max_output_tokens`) rather than a fixed constant.

## Design

Compaction budget is **derived at deps-assembly time from the primary model's
real context window**. No harness changes, no extra LLM calls, no per-turn model
threading (R10 thin-harness safe). The existing pressure sensor / compactor are
untouched — they simply receive a model-aware `token_budget`.

### 1. Config surface — one new field

`ProviderConfig` (`src/config/types/provider.rs`) gains:

```rust
/// Operator-declared total context window (tokens) for this provider's
/// model(s). Escape hatch for third-party endpoints (302ai / moonshot /
/// openrouter) whose effective window differs from, or is absent in, the
/// static capability catalog. When unset, the catalog (then a default) is used.
#[serde(default)]
pub context_window: Option<u32>,
```

The existing `max_tokens: Option<u32>` ("Maximum tokens in response") is **reused**
as the output-reserve source — no new field for that.

```toml
[providers.302ai]
models = ["claude-sonnet-4-6"]
context_window = 1000000   # operator declares 1M-beta access
max_tokens = 64000         # already exists → reused as output reserve
```

### 2. Derivation function (pure)

New `derive_token_budget(primary: &ProviderConfig, model: &str) -> u64` (in
`deps_builder.rs`, near `build_context_budget_config`):

- **window** = `provider.context_window`
  ▸ `capabilities_for(model).context_window`
  ▸ `DEFAULT_CONTEXT_TOKEN_BUDGET` (200_000)
- **reserve** = `provider.max_tokens`
  ▸ `capabilities_for(model).max_output_tokens`
  ▸ `DEFAULT_OUTPUT_RESERVE` (8_192)
- **usable** = `window.saturating_sub(reserve)`, clamped to a minimum floor
  `MIN_USABLE_BUDGET` (e.g. 16_384) so a misdeclared tiny window or
  reserve ≥ window never yields a zero/absurd budget.

The existing `warning_threshold` (0.70) / `critical_threshold` (0.85) remain
**global fractions** applied to this per-model `usable` budget — so the *absolute*
trigger points differ per model automatically, with no per-model threshold knobs.

### 3. Wiring (precedence preserves back-compat)

`build_context_budget_config` signature extends to receive the primary provider
config + primary model id (the primary provider key is already computed by
`build_failover_chain`; the model is `providers[key].models.first()`).

```text
if [context_budget].token_budget is explicitly set  -> use it verbatim   (current behavior, unchanged)
else                                                 -> derive_token_budget(primary, model)
```

A config with an explicit `token_budget` behaves exactly as today.

### 4. Catalog extension

Add curated entries to `capabilities.rs` for **Kimi/Moonshot, GLM (Zhipu),
Qwen** (best-effort vendor figures, same "operators upgrade Aleph to refresh"
stance as existing entries).

**Claude stays at 200k** in the catalog (1M is beta and header-gated — cannot be
assumed for all `sonnet-4` ids). Operators with 1M access declare
`context_window = 1000000` on their provider — exactly the escape hatch's purpose.

### 5. Observability

At startup, emit one line mirroring `failover chain assembled`:

```
context budget derived: model=<id> window=<w> reserve=<r> usable=<u> source=<config|catalog|default>
```

### 6. Testing

- `derive_token_budget`: config-window wins; catalog fallback; default fallback;
  reserve precedence (provider.max_tokens > catalog > default); `saturating_sub`
  + min-floor clamp when reserve ≥ window.
- `build_context_budget_config`: explicit `token_budget` overrides derivation;
  omitted `token_budget` derives per model; disabled → `None` (unchanged).
- catalog: new Kimi/GLM/Qwen entries resolve with expected windows.

## Redline / principle check

- **R10 (thin harness):** zero changes under `src/harness/`; build-time + static
  data only; no reasoning, no extra LLM round-trips.
- **R3 / P6 (minimalism, KISS/YAGNI):** one new config field, one reused field,
  one new pure function, one data-table extension. No per-turn dynamic plumbing.
- **R7 (LLM sovereignty):** the catalog is data the LLM/operator consult, not a
  router — nothing here auto-selects a model.

## Deferred (explicitly out of scope)

- **Dynamic per-turn budget** following failover model switches (would require
  threading the live model through the `AiProvider` boundary — invasive,
  previously deferred). Build-time primary-model derivation is conservative-safe:
  switching to a *larger* window just compacts earlier than strictly necessary.
- **Min-over-chain** budget (take the smallest window across the failover chain).
  Considered, rejected for this pass: penalizes the common single/primary-model
  case.
- Per-model `warning`/`critical` threshold overrides (current global fractions
  already yield per-model absolute trigger points).
