# Classification & Routing Matrix: PROVIDERS_CATALOG_FAILOVER

## Executive Summary

- **Subsystem**: Providers Model Catalog, Failover, and Presets (`src/providers/failover/`, `src/providers/model_catalog/`, `src/providers/presets/`)
- **Assigned Files**:
  - `src/providers/failover/decision.rs`
  - `src/providers/failover/health.rs`
  - `src/providers/failover/mod.rs`
  - `src/providers/failover/provider.rs`
  - `src/providers/failover/tests.rs`
  - `src/providers/model_catalog/alias.rs`
  - `src/providers/model_catalog/capabilities.rs`
  - `src/providers/model_catalog/discovery.rs`
  - `src/providers/model_catalog/drift_tests.rs`
  - `src/providers/model_catalog/endpoint.rs`
  - `src/providers/model_catalog/lifecycle.rs`
  - `src/providers/model_catalog/mod.rs`
  - `src/providers/model_catalog/record.rs`
  - `src/providers/presets/mod.rs`
  - `src/providers/presets/registry.rs`
  - `src/providers/presets/tests.rs`
- **Assessment**:
  The `failover`, `model_catalog`, and `presets` subsystems implement critical resilience and discovery logic: circuit breaker states, proactive rate pacing, prefix-based model capability lookups, locality classification, and provider profiles.
  Static analysis detected multiple redundant heap allocations on hot lookup paths (e.g. repeated `to_lowercase()`, `to_string()`, and intermediate `Vec` allocations in lookup and filter paths), process-wide write-lock contention in the circuit breaker, and accidental complexity in manual IP parsing and input overrides.
  In total, 19 candidate smells were identified:
  - **11 `FIX-NOW`**: High-confidence local simplifications eliminating ~12 heap allocations per call, replacing hand-rolled IP parsing with `std::net::Ipv4Addr`, inlining single-use helpers, and removing intermediate filter vectors.
  - **3 `FIX-CAREFULLY`**: Concurrency- and contract-sensitive optimizations (optimistic read locking in `circuit_allows`, borrowable key lookup in `ModelCooldown::is_cooling`, and slice-based signature in `effective_fallback_names`).
  - **5 `REJECT`**: Deliberately retained patterns where changes would cross subsystem/crate boundaries (`retain_capable_models`, `canonicalize_model_id`), break intentional fail-open resilience (`drop_cooling_models`), or disrupt single-source-of-truth invariants (`model_ladder` -> `model_roster`).

---

## Classification & Routing Matrix

| Location | Taxonomy | Smell Description | Route | Action |
|:---|:---|:---|:---:|:---|
| `src/providers/failover/decision.rs:198-202` | `CONTROL-FLOW / INDIRECTION` | `next_provider` is eagerly evaluated via `is_permanent_failure(&msg)` before matching `classify_exhausted`, even when the error is fatal or compactable. | `REJECT` | Retain as-is. Eager construction of a 2-variant Copy enum avoids duplicating fallback logic across multiple match arms; `is_permanent_failure` cost on the failure path is negligible. |
| `src/providers/failover/health.rs:214-221` | `REDUNDANT-ALLOCATION` | `ModelCooldown::is_cooling` allocates two heap `String`s via `(provider.to_string(), model.to_string())` on every candidate check to query `HashMap<(String, String), Instant>`. | `FIX-CAREFULLY` | Refactor `ModelCooldown` internal map key to a custom key struct with `Borrow<(&str, &str)>`, eliminating two heap allocations per model candidate check per turn. |
| `src/providers/failover/health.rs:270-272` | `REDUNDANT-ALLOCATION` | `ProviderCooldown::cool` calls `map.entry(provider.to_string()).or_insert(until)`, allocating a heap `String` even when `provider` is already present in `map`. | `FIX-NOW` | Check `if let Some(slot) = map.get_mut(provider)` first to update `*slot = (*slot).max(until);`, only allocating `provider.to_string()` when inserting a new entry. |
| `src/providers/failover/provider.rs:74-96` | `REDUNDANT-ALLOCATION` | `effective_fallback_names` calls `configured_minus_primary()`, creating an intermediate `Vec<String>`, then immediately runs `.into_iter().filter(|n| live.contains(n.as_str())).collect()`. | `FIX-NOW` | Combine into a single iterator pass: `configured_names.iter().filter(\|n\| n.as_str() != primary_name && live.contains(n.as_str())).cloned().collect()`, eliminating the intermediate `Vec<String>` heap allocation. |
| `src/providers/failover/provider.rs:326` | `REDUNDANT-ALLOCATION` | `fallback_nodes` allocates an owned `Vec<String>` via `self.fallbacks.iter().map(\|n\| n.name.clone()).collect()` solely to pass `&[String]` to `effective_fallback_names`. | `FIX-CAREFULLY` | Change `effective_fallback_names` to take `configured_names: &[impl AsRef<str>]` (or `&[&str]`), allowing `self.fallbacks` to be passed without cloning all provider names into a temporary `Vec`. Update call sites in `tests.rs`. |
| `src/providers/failover/provider.rs:359-372` | `CONTROL-FLOW / INDIRECTION` | `circuit_allows` unconditionally acquires a process-wide `write().await` lock on `self.health.0` and calls `entry(name.to_string())`, causing lock contention and heap allocation for every candidate on every request (even in healthy `Closed` state). | `FIX-CAREFULLY` | Implement two-phase check: first acquire `read().await` lock to check if entry is absent or `Closed`/`HalfOpen`, returning `true` with zero allocations; acquire `write().await` lock only when transitioning `Open -> HalfOpen` after cooldown has elapsed. |
| `src/providers/failover/provider.rs:375-382` | `REDUNDANT-ALLOCATION` | `mark_healthy` calls `map.entry(name.to_string()).or_default()` under `write().await` lock, allocating `name.to_string()` on every successful request even when `name` is already in the map. | `FIX-NOW` | Check `if let Some(st) = map.get_mut(name)` to update in-place without allocating `name.to_string()`; allocate only when inserting a new entry. |
| `src/providers/failover/provider.rs:268-285` | `REDUNDANT-ALLOCATION` | `drop_cooling_models` takes `models: Vec<Option<String>>` by value but iterates `for m in &models` and clones each item (`kept.push(m.clone())`). | `REJECT` | Retain as-is. `drop_cooling_models` must preserve the original `models` vector for the fail-open fallback if all models are cooling, and `is_cooling` is async so `models.retain()` cannot be used. Model vectors are small (1-3 items). |
| `src/providers/failover/provider.rs:640-658` | `REDUNDANT-ALLOCATION` | `cand.models.clone()` in `walk` when resolving model list with `retain_capable_models`. | `REJECT` | Retain clone. `retain_capable_models` is defined in `src/providers/capability_gate.rs` (`PROVIDERS_CORE` batch) and takes owned `Vec<String>`. Altering its signature would cross subsystem boundaries. |
| `src/providers/model_catalog/alias.rs:110` | `REDUNDANT-ALLOCATION` | `canonical_provider_id` calls `provider.trim().to_ascii_lowercase()`, allocating a new `String` on every alias resolution even when `provider` is already lowercase and trimmed. | `FIX-NOW` | Use `Cow<'_, str>`: if `provider.trim()` contains no uppercase ASCII bytes, borrow directly as `Cow::Borrowed`; only allocate `Cow::Owned` when uppercase characters are present. |
| `src/providers/model_catalog/capabilities.rs:1578-1584` | `REDUNDANT-ALLOCATION` | `capabilities_for` and `binds_thinking_to_prefix` call `canonicalize_model_id(model)`, allocating an owned `String`. | `REJECT` | Retain as-is. `canonicalize_model_id` lives in `aleph-slot-api` crate and returns `String` because it strips vendor tags, date stamps, and normalizes separators. Modifying it would break cross-crate contract. |
| `src/providers/model_catalog/discovery.rs:178` | `REDUNDANT-ALLOCATION` | `refresh_models` accesses `REFRESH_LOCKS` via `locks.entry(provider.to_string()).or_insert_with(...)`, allocating `provider.to_string()` on every refresh even when the lock entry already exists. | `FIX-NOW` | Check `if let Some(lock) = locks.get(provider)` first, cloning only the `Arc` without allocating `provider.to_string()`. |
| `src/providers/model_catalog/endpoint.rs:91-102` | `REDUNDANT-ALLOCATION` | `extract_host` allocates an owned `String` via `raw.to_string()` when `raw.contains("://")`. | `FIX-NOW` | Use `Cow::Borrowed(raw)` when `raw.contains("://")`, avoiding heap allocation for standard URL strings. |
| `src/providers/model_catalog/endpoint.rs:140-178` | `DEAD-CODE / ACCIDENTAL-COMPLEXITY` | `host_is_local` uses a hand-rolled `parse_ipv4` parser and 20 lines of manual octet comparisons, duplicating functionality provided by `std::net::Ipv4Addr`. | `FIX-NOW` | Replace `parse_ipv4` and octet checks with `if let Ok(v4) = host.parse::<std::net::Ipv4Addr>() { return v4.is_loopback() \|\| v4.is_private() \|\| v4.is_link_local(); }`, and delete `parse_ipv4`. |
| `src/providers/model_catalog/record.rs:149, 160-168` | `DEAD-CODE / ACCIDENTAL-COMPLEXITY` | `apply_input_override` is a redundant private helper with an unused `_current: &bool` parameter that merely extracts `input.image`. | `FIX-NOW` | Inline `if let Some(input) = def.input.as_ref() { caps.supports_vision = input.image; }` and delete `apply_input_override`. |
| `src/providers/presets/mod.rs:250-252` | `REDUNDANT-ALLOCATION` | `provider_metadata` unconditionally allocates a lowercase `String` via `name.to_lowercase()`. | `FIX-NOW` | Fast-path lookup `if let Some(m) = PRESET_METADATA.get(name) { return Some(m); }` before calling `to_lowercase()`, avoiding string allocation for canonical lowercase names. |
| `src/providers/presets/mod.rs:273-276` | `REDUNDANT-ALLOCATION` | `get_preset` unconditionally allocates a lowercase `String` via `name.to_lowercase()`. | `FIX-NOW` | Fast-path lookup `if let Some(p) = PRESETS.get(name) { return Some(p); }` before calling `to_lowercase()`, avoiding string allocation for canonical lowercase names. |
| `src/providers/presets/registry.rs:1076-1078` | `REDUNDANT-ALLOCATION` | `canonical_preset_id` unconditionally allocates a lowercase `String` via `name.to_lowercase()`. | `FIX-NOW` | Fast-path lookup `if let Some(id) = CANONICAL_ID.get(name) { return Some(*id); }` before calling `to_lowercase()`, avoiding string allocation for canonical lowercase names. |
| `src/providers/presets/mod.rs:302-307` | `REDUNDANT-ALLOCATION` | `model_ladder` delegates to `model_roster`, resolving full `ModelRecord`s (capabilities, cost, lifecycle) only to discard all metadata and retain `id: String`. | `REJECT` | Retain delegation. `model_roster` is the single source of truth for model ladder resolution and fallback deduplication; separating ladder resolution would create duplicate resolution logic and risk ordering drift. |

---

## Invariant Safety Pre-Flight

1. **Failover Circuit Breaker & Rate Pacing Invariants** (`failover/health.rs`, `failover/provider.rs`):
   - Concurrency semantics: The three-state breaker (`Closed -> Open -> HalfOpen`) and state transitions (`Open -> HalfOpen` only after cooldown expiry) remain strictly atomic and thread-safe.
   - Pacing guarantee: `ProviderCooldown::cool` continues to take `max(existing, new)`, ensuring a longer server `Retry-After` is never clobbered by a shorter default.
   - Fail-open fallback: The terminal candidate in `walk` must always be dialed even if its circuit is open, preventing transient cascading outages from completely blocking requests.
   - Fail-open model filtering: `drop_cooling_models` continues to preserve the original model list if all candidate models are cooling.

2. **Model Catalog Capabilities & Normalization Invariants** (`model_catalog/capabilities.rs`, `model_catalog/alias.rs`, `model_catalog/endpoint.rs`):
   - Prefix precedence: Declaration order in `CAPABILITY_TABLE` and `MODEL_VENDOR_PREFIXES` remains strictly preserved so specific models always precede broad family entries.
   - Locality classification: `endpoint_kind_for_base_url` maintains byte-identical classification for loopback (`127.*`, `localhost`, `::1`), RFC-1918 private ranges (`10.*`, `172.16-31.*`, `192.168.*`), link-local (`169.254.*`, `fe80::/10`), and unique-local (`fc00::/7`).
   - Single join point: `ModelRecord::resolve` remains the sole join function across capabilities, cost, endpoint locality, and lifecycle data.

3. **Presets Registry & Fallback Order Invariants** (`presets/mod.rs`, `presets/registry.rs`):
   - Curated fallback ordering: `model_roster` and `model_ladder` preserve exact resolution precedence: explicit base models first -> preset fallback rungs -> discovered long-tail models.
   - Case-insensitive resolution: All preset and alias lookups remain case-insensitive while fast-pathing lowercase matches.
   - Temperature policy: `temperature_for_base_url` maintains exact pass-through, omission, or fixed-value enforcement without breaking custom base URL exemptions.
