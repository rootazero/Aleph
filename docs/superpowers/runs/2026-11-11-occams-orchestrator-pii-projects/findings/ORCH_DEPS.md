# Occam's Razor Remediation: Batch ORCH_DEPS (`src/orchestrator/deps_builder/`)

## 1. Summary of Changes
- **`src/orchestrator/deps_builder/mod.rs`**: Gated `mod common;` with `#[cfg(test)]`.
- **`src/orchestrator/deps_builder/common.rs`**: Replaced imperative HashMap insertion loop in `cfg_with_fallback` with iterator `.collect()`.
- **`src/orchestrator/deps_builder/context_budget.rs`**:
  - Flattened `derive_summarizer_input_budget` control flow using `if let Some((model, _)) = summary_model`.
  - In `derive_chain_min_budget`, switched `keys` to `Vec<&str>` and eliminated temporary heap `Vec<Option<&str>>` per provider, evaluating model windows with an in-place closure.
  - In `build_context_budget_refiner`, merged two separate iterations and lowercasing passes into a single combined loop over `config.providers`.
  - In `refine_for_serving_model`, short-circuited `serving_provider.eq_ignore_ascii_case(&self.primary_provider_key)`, eliminating two heap `String` allocations per turn in standard execution.
- **`src/orchestrator/deps_builder/provider_chain.rs`**:
  - Refactored `protocol_of` to return `Option<&str>`, eliminating allocations during fallback protocol affinity sorting.
  - Removed redundant identity `.map(|(k, v)| (k, v))` closure on `built.iter().find(...)`.
- **`src/orchestrator/deps_builder/stability.rs`**: Derived `Default` on `StabilityTriple`, simplifying the early return to `StabilityTriple::default()`.
- **`src/orchestrator/deps_builder/summary.rs`**:
  - Changed `build_provider_with_swapped_model` to accept `new_model: &str`, allocating once inside and eliminating caller-side clones.
  - Refactored `effective_summary_model` to return `Option<(&'a str, &'static str)>` borrowing from `&'a Config`, and passed `primary_provider_key` directly to `get_preset` without redundant `.to_lowercase()`.
  - Kept `dream_model` and `planner_model` as `&str` directly in `build_dream_provider` and `build_strategy_planner_provider`, eliminating multiple intermediate allocations and 4 `excessive-clone` suppressions.

## 2. Invariant Safety Pre-Flight
- Token budget math, clamping bounds, and headrooms (`WARNING_SPIKE_HEADROOM_TOKENS`) preserved.
- Failover chain discovery order and protocol affinity ranking remain deterministic.
- Distinct model resolution semantics across summary, dream, and strategy planners preserved without premature consolidation.
