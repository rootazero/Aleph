# Classification & Routing Matrix: ORCH_DEPS

## Executive Summary

- **Subsystem**: Orchestrator Dependency Builders (`src/orchestrator/deps_builder/`)
- **Assigned Files**:
  - `src/orchestrator/deps_builder/common.rs`
  - `src/orchestrator/deps_builder/context_budget.rs`
  - `src/orchestrator/deps_builder/mod.rs`
  - `src/orchestrator/deps_builder/provider_chain.rs`
  - `src/orchestrator/deps_builder/stability.rs`
  - `src/orchestrator/deps_builder/summary.rs`
- **Assessment**:
  The `deps_builder` module is well-tested and robust in its arithmetic and defensive checks. However, multiple modules exhibit substantial **accidental allocation patterns** and **gratuitous `.clone()` / `.to_string()` calls**, particularly on the per-turn / per-run execution paths (`refine_for_serving_model`, `protocol_of` in fallback sorting, `build_provider_with_swapped_model`, and `derive_chain_min_budget`).
  In total, 19 candidate smells were classified:
  - **14 `FIX-NOW`**: High-confidence local simplifications eliminating ~16 heap allocations/clones, removing redundant identity closures, and cleaning up test module gating.
  - **5 `REJECT`**: Deliberately rejected refactorings where code changes would violate subsystem encapsulation (e.g. mutating `FailoverProvider` or `model_ladder` across crate boundaries), disrupt intentional `Arc` concurrency primitives, or introduce premature abstractions that couple independent domain concepts (e.g. dreaming vs strategy planner model resolution).

---

## Classification & Routing Matrix

| Location | Taxonomy | Smell Description | Route | Action |
|:---|:---|:---|:---:|:---|
| `src/orchestrator/deps_builder/mod.rs:8` | `DEAD-CODE / ACCIDENTAL-COMPLEXITY` | `mod common;` is declared unconditionally, but `common.rs` is an entirely test-gated (`#[cfg(test)]`) module. In non-test/release builds, it emits an empty compilation unit. | `FIX-NOW` | Gate module declaration with `#[cfg(test)] mod common;` in `mod.rs`. |
| `src/orchestrator/deps_builder/common.rs:13-17` | `REDUNDANT-ALLOCATION` | `cfg_with_fallback` populates `providers_map` via an imperative `for (k, v) in providers { providers_map.insert(k.to_string(), v); }` loop instead of an iterator collection. | `FIX-NOW` | Replace imperative loop with `providers.into_iter().map(|(k, v)| (k.to_string(), v)).collect()`. |
| `src/orchestrator/deps_builder/context_budget.rs:98-111` | `CONTROL-FLOW / INDIRECTION` | `derive_summarizer_input_budget` wraps `summary_model` in `.as_ref().map(|(model, _)| model.as_str())` before matching `Some(model)` and `None`. | `FIX-NOW` | Flatten control flow using `if let Some((model, _)) = summary_model` directly to resolve `capabilities_for(model)`. |
| `src/orchestrator/deps_builder/context_budget.rs:323-328` | `REDUNDANT-ALLOCATION` | `derive_chain_min_budget` allocates owned strings in `keys = vec![primary_provider_key.to_string()]` and clones fallback names via `keys.push(name.clone())` just to perform map lookups. | `FIX-NOW` | Use `Vec<&str>` (`vec![primary_provider_key]`, `keys.push(name.as_str())`), avoiding multiple intermediate `String` allocations during chain-min derivation. Only allocate `provider: (*key).to_string()` when storing the winning budget. |
| `src/orchestrator/deps_builder/context_budget.rs:338-342` | `REDUNDANT-ALLOCATION` | `derive_chain_min_budget` allocates a temporary `Vec<Option<&str>>` on the heap for every provider in the failover chain just to iterate over candidate model windows once. | `FIX-NOW` | Iterate over `&p.models` (or single `None` evaluation) directly via a loop/closure without allocating an intermediate `Vec`. |
| `src/orchestrator/deps_builder/context_budget.rs:601-610` | `REDUNDANT-ALLOCATION` | `build_context_budget_refiner` iterates `config.providers` twice and calls `name.to_ascii_lowercase()` on both passes to construct `provider_windows` and `provider_max_tokens`. | `FIX-NOW` | Combine into a single pass over `config.providers`, lowercasing `name` once per provider that has declared window or max tokens. |
| `src/orchestrator/deps_builder/context_budget.rs:666-667` | `REDUNDANT-ALLOCATION` | `refine_for_serving_model` allocates two heap `String`s via `.to_ascii_lowercase()` on every call (per run) just to check `is_primary`. In typical runs where the serving provider is primary, `serving_key` is unused. | `FIX-NOW` | Check `serving_provider.eq_ignore_ascii_case(&self.primary_provider_key)` (zero allocations); compute lowercase `serving_key` only in the non-primary `else` branch for fallback lookups. |
| `src/orchestrator/deps_builder/context_budget.rs:621-623` | `CONTROL-FLOW / INDIRECTION` | `primary_window_override(&self, window_override)` is a 3-line helper with an unused `&self` receiver called in exactly one branch. | `REJECT` | Retain helper. The helper isolates the failover-chain wrapper contract and carries a dedicated doc comment explaining why the primary window override is passed through for the failover route (P5 / Occam's Razor: keep tiny function when it isolates an invariant). |
| `src/orchestrator/deps_builder/provider_chain.rs:57-71` | `REDUNDANT-ALLOCATION` | `pc.all_models().to_vec()` clones the model slice into an owned `Vec<String>` before calling `model_ladder`. | `REJECT` | Retain as-is. `model_ladder` lives in `src/providers/presets/mod.rs` (outside `ORCH_DEPS` batch) and takes owned `Vec<String>`. Modifying its public contract would cross crate/subsystem boundaries. |
| `src/orchestrator/deps_builder/provider_chain.rs:145-202` | `REDUNDANT-ALLOCATION` | Multiple `.clone()` calls across `FailoverProvider::new` arguments (`model_catalog`, `health`, `failover_config`, `model_cooldown`, `provider_cooldown`, `global`). | `REJECT` | Retain clones. `FailoverProvider` takes owned `model_catalog` and `failover_config`; modifying `FailoverProvider` crosses subsystem boundaries into `src/providers/failover/`. Arc clones (`health`, `global`, etc.) represent intentional shared concurrency primitives and are O(1) atomic ref-count increments. |
| `src/orchestrator/deps_builder/provider_chain.rs:264-272, 360` | `REDUNDANT-ALLOCATION` | `protocol_of` allocates a `String` on every lookup and every comparison inside `names.sort_by_key(...)`. | `FIX-NOW` | Change `protocol_of` to return `Option<&str>` via `pc.protocol.as_deref().unwrap_or("openai")`, eliminating heap allocations during protocol affinity checks and sorting. |
| `src/orchestrator/deps_builder/provider_chain.rs:315-318` | `DEAD-CODE / ACCIDENTAL-COMPLEXITY` | Identity mapping `.map(|(k, v)| (k, v))` on `built.iter().find(...)`. | `FIX-NOW` | Remove redundant `.map(|(k, v)| (k, v))` and pattern match directly on `built.iter().find(...)`. |
| `src/orchestrator/deps_builder/stability.rs:12-36` | `DEAD-CODE / ACCIDENTAL-COMPLEXITY` | `StabilityTriple` lacks `Default` derive, forcing manual `None` initialization of all three optional fields in `build_stability_triple`. | `FIX-NOW` | Derive `Default` on `StabilityTriple` and simplify early exit to `return StabilityTriple::default();`. |
| `src/orchestrator/deps_builder/summary.rs:18, 25` | `REDUNDANT-ALLOCATION` | `build_provider_with_swapped_model` takes `new_model: String` and clones it inside (`cfg.models = vec![new_model.clone()]`). Callers at lines 141, 214, 280 also clone their strings (`summary_model.clone()`, `dream_model.clone()`, `planner_model.clone()`), causing double-clones. | `FIX-NOW` | Change `new_model` parameter to `&str` in `build_provider_with_swapped_model`. Allocate `cfg.models = vec![new_model.to_string()]` once inside; remove caller-side `.clone()`s (eliminating 4 `.clone()` calls across the module). |
| `src/orchestrator/deps_builder/summary.rs:94-120` | `REDUNDANT-ALLOCATION` | `effective_summary_model` returns `Option<(String, &'static str)>`, allocating `String`s for `model` and calling `m.clone()` on matching `base.models.first()`. | `FIX-NOW` | Change return type to `Option<(&'a str, &'static str)>` borrowing from `&'a Config`. Eliminate `model.to_string()` and `m.clone()`. |
| `src/orchestrator/deps_builder/summary.rs:108, 189` | `REDUNDANT-ALLOCATION` | `crate::providers::get_preset(&primary_provider_key.to_lowercase())` allocates a temporary lowercase `String`, while `get_preset` itself already performs `.to_lowercase()` internally on line 274. | `FIX-NOW` | Pass `primary_provider_key` directly to `get_preset(primary_provider_key)`, eliminating redundant string allocation at both call sites. |
| `src/orchestrator/deps_builder/summary.rs:182-193` | `REDUNDANT-ALLOCATION` | `build_dream_provider` allocates a `String` for `dream_model` from either `explicit` (`model.to_string()`) or `p.default_aux_model` (`...?.to_string()`), both of which are already borrowed string slices. | `FIX-NOW` | Keep `dream_model` as `&str` directly from `match explicit { Some(m) => m, None => ... }` with zero string allocations. |
| `src/orchestrator/deps_builder/summary.rs:258-264` | `REDUNDANT-ALLOCATION` | `build_strategy_planner_provider` calls `.to_string()` on `planner_model` (`&str`), then clones it (`planner_model.clone()`), which is cloned yet again in `build_provider_with_swapped_model`. | `FIX-NOW` | Keep `planner_model` as `&str` and pass directly to `build_provider_with_swapped_model`, eliminating 2 string allocations. |
| `src/orchestrator/deps_builder/summary.rs:182-218` | `CONTROL-FLOW / INDIRECTION` | Structurally similar two-tier model resolution across summary, dream, and strategy planners. | `REJECT` | Reject consolidating into a unified resolution macro/trait. Each builder has distinct semantic domain invariants: summary allows auto-aux fallback and logs source; dream requires dreaming to be enabled and uses its own config section; planner explicitly prohibits aux fallback (reasoning task). Unifying them would introduce artificial coupling and indirection (violating Occam's Razor: "Prefer a clear local duplication over a premature abstraction"). |

---

## Invariant Safety Pre-Flight

1. **Token Budget Arithmetic & Reservation Windows** (`context_budget.rs`):
   - The arithmetic in `usable_budget`, `window_aware_warning_default`, and `window_aware_fresh_tail` remains strictly byte-identical.
   - The 48,000 token spike headroom invariant (`WARNING_SPIKE_HEADROOM_TOKENS`) is preserved without modification.
   - Refinement monotonic guarantee: `refine_for_serving_model` continues to enforce `min(base.token_budget, serving_usable)` — it can only compact earlier, never later than the startup safety floor.

2. **Provider Failover & Routing Invariants** (`provider_chain.rs`):
   - Preserves canonical `[fallback_provider].chain` precedence over legacy `[general] fallback_providers`.
   - Protocol affinity ordering in auto-derived fallbacks (same-protocol first, name-sorted tiebreak) remains deterministic and byte-identical.
   - Shared circuit-breaker (`FailoverHealth`), per-model cooldown (`ModelCooldown`), and per-provider cooldown (`ProviderCooldown`) remain shared singletons across all chains.
   - Live primary lookup (`default_provider.current()`) remains hot-swappable per request.

3. **Cheap Tier & Planner Reasoning Invariants** (`summary.rs`):
   - Cheap summary tier preserves exact resolution precedence: explicit `summary_model` → preset `default_aux_model` → main LLM fallback.
   - Memory dreaming tier preserves explicit `dreaming.model` → preset `default_aux_model` → main LLM fallback, and strictly respects `dreaming.enabled`.
   - Strategy planner tier preserves its hard invariant: strictly NO aux-tier fallback (planner model unset/blank reuses the main reasoning executor, never flash-tier).
