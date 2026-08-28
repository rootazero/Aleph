# Logic Review Report — src/providers (CORE)
**Date**: 2026-08-28
**Mode**: strict
**Scope**: top-level `.rs` files only (no subdirectory recursion)
**Reviewer**: sub-agent (focused re-dispatch after 429 rate limit on previous run)

## Files reviewed

`adapter.rs`, `capability_gate.rs`, `catalog.rs`, `default_handle.rs`, `delta.rs`,
`health.rs`, `http_provider.rs`, `llm_retry.rs`, `load_stats.rs`, `message.rs`,
`metadata.rs`, `metering.rs`, `mock.rs`, `model_override_provider.rs`, `mod.rs`,
`probe.rs`, `recording_mock.rs`, `registry.rs`, `retry.rs`, `route_handle.rs`,
`route_observe.rs`, `route_policy.rs`, `route_witness.rs`, `session_moa_handle.rs`,
`session_model_handle.rs`, `think_level_provider.rs`

---

## Findings

### [Verified — no issue] `route_policy.rs:422` `(rr_base % g.len() as u64) as usize` is safe
- **Location**: `src/providers/route_policy.rs:422` inside `balance_group`
- **Prior concern (carried forward)**: "if `g.is_empty()`, division by zero"
- **Actual behaviour**: The function short-circuits with `if group.len() <= 1 { return group; }` on line 415, so `group.len()` is guaranteed to be ≥ 2 before the modulo. On every platform `usize → u64` is a lossless widening cast. No division-by-zero hazard.
- **Verdict**: **NO ACTION** — the partial-finding file `route_policy.rs:422` flagged "verify if guarded" is now resolved (it is guarded).

### [Verified — no issue] `route_handle.rs:382` `unreachable!("neither config uses Auto")` is in test code
- **Location**: `src/providers/route_handle.rs:382`
- **Prior concern (carried forward)**: "`RouteMode::Auto => unreachable!(...)` in production"
- **Actual behaviour**: Line 382 sits inside the `fn concurrent_store_is_never_observed_torn` test function (verified by reading the test fn body, which only writes `cfg_a = AlwaysLocal` and `cfg_b = AlwaysCloud`). The match is exhaustive over the test's two configs — `Auto` genuinely is unreachable for this test.
- **Verdict**: **NO ACTION** — the unreachable is in `#[cfg(test)]` and load-bearing for the test's invariant. Prior audit was wrong about "production".

### [Verified — no issue] All other `panic!`/`unreachable!` in src/providers/ are in `#[cfg(test)]`
- **Locations**: `http_provider.rs:811,818`, `llm_retry.rs:989`, `message.rs:511,522,536,547,566,582,613,618,695,723,856,858`, `metering.rs:475,533,791`, `mock.rs:219`, `registry.rs:258`
- **Verified**: every hit from `grep -n "unreachable\|panic!" src/providers/*.rs | grep -v "cfg(test)"` is inside a `#[cfg(test)]` module or `#[cfg(test)]` function. The bare `writer.join().unwrap()` / `r.join().unwrap()` on lines 389, 392 of `route_handle.rs` are test-thread joins.
- **Verdict**: **NO ACTION**.

### [Warning] `route_observe.rs:141` price cast could panic on non-finite f64
- **Location**: `src/providers/route_observe.rs:141` in `fn price_milli_per_mtok`
- **Risk**: `(usd * 1000.0).round() as u64` performs an `f64 → u64` `as` cast. If `usd` is `NaN`, the cast yields `0` on stable Rust (saturating). If `usd` is `+Inf`, the cast yields `u64::MAX` (also saturating). If `usd` is negative, the cast saturates to `0`. So the cast itself does NOT panic in current Rust (the saturating-cast semantics landed in 1.45). However the **`round()` call on NaN returns NaN**, and `NaN as u64` is `0` (defined behaviour).
- **Source values**: `card.input_per_mtok.unwrap_or(0.0) + card.output_per_mtok.unwrap_or(0.0)`. Both come from a static rate card table (`src/pricing.rs`). The values are typed `Option<f64>` and could in principle carry any f64 the table author wrote — including `NaN`/`Inf` if a hand-edited table had a typo.
- **Current impact**: Low — today's static table contains only finite values, so the path is safe. But the function does no defence-in-depth: a future rate-card entry of `f64::INFINITY` would silently yield `u64::MAX`, which would make an effectively-free-looking unpriced-cloudsort sentinel collide with `CostAware`'s "cost unknown" rank (which is exactly the position the sentinel is meant to occupy — confusing but not corrupting).
- **Suggestion**: Either (a) clamp the input with `if !usd.is_finite() { return None; }` to refuse an obviously-wrong card entry, or (b) document in the function's doc that a non-finite rate is treated as "unpriced".

### [Warning] `catalog.rs::CatalogEntry::supports` returns false for chat-kind entries with non-Chat modalities when metadata is missing
- **Location**: `src/providers/catalog.rs` `impl CatalogEntry::supports` (~line 88)
- **Risk**: The fallthrough branch is `None => self.kind == CatalogKind::Chat && modality == Modality::Chat`. Any chat preset that lacks explicit `ProviderMetadata` is treated as **only** capable of `Chat`. If a future chat-side preset adds e.g. vision support without registering metadata, `presets_for_modality(Modality::Image)` silently omits it.
- **Current impact**: Low — every production chat preset registers metadata (covered by `catalog_entry_supports_default_chat_for_chat_without_metadata` test on the synthetic case). But the omission is invisible at the call site; no log line.
- **Suggestion**: Add a `tracing::debug!` for the `kind == Chat && modality != Chat && metadata.is_none()` branch so a missing metadata entry shows up in logs rather than only in panel output. This is a "fail-open silently" smell, not a correctness bug.

### [Warning] `capability_gate.rs::RequestRequirements::from_request` overestimates tokens for tool-heavy requests
- **Location**: `src/providers/capability_gate.rs:108` (`from_request`)
- **Risk**: The token estimate is `chars / 4` over `m.text_content()`, which serializes **every** content block including `ContentBlock::ToolCall { name, arguments, .. }` as `format!("{name} {arguments}")` (see `message.rs::text_content`). For a request with several large tool-call arguments, this overcounts by ~50% (JSON adds braces, quotes, commas — much denser than prose), which makes the context-window gate drop more candidates than necessary.
- **Current impact**: Low — the gate is fail-open (returns the original list when it would empty the chain), and an over-conservative gate is the cheap failure. But a request with exactly 5 candidates where 3 are dropped because of overcounting is a noticeable false negative.
- **Suggestion**: Either (a) document the heuristic explicitly ("intentionally pessimistic; see `from_request` doc"), or (b) exclude `ToolCall` blocks from the estimate (the harness separately tokenizes them). The current doc does say "intentionally rough", but the failure mode isn't named.

### [Warning] `metering.rs` uses `chrono::Utc::now()` for spend ceiling, not monotonic time
- **Location**: `src/providers/metering.rs:127` (`enforce_spend_ceiling`), `metering.rs:170` (`record_spend_with`)
- **Risk**: Both arms of the spend floor use wall-clock time. An NTP step backward can cause `chrono::Utc::now().timestamp_millis()` to decrease, which `spend::check_with` and `spend::period::period_start_ms` must then tolerate. By contrast, `load_stats.rs::now_secs()` deliberately uses `Instant::now()` for exactly this reason — see its module doc.
- **Current impact**: Low — wall-clock is correct for "what period is this hour/week/month in" semantics. The risk is only if a backward NTP step causes the same call to be charged against two periods (unlikely with 1ms precision but possible with a 1-hour step).
- **Suggestion**: Document the wall-clock choice in `metering.rs` module doc, mirroring the `load_stats.rs` justification. No code change needed — wall-clock is the right primitive for human-meaningful period boundaries.

### [Suggested Test] `delta::salvage_malformed_args` Unicode boundary coverage
- **Location**: `src/providers/delta.rs:442-465` (`repair_json_emission_defects`)
- **Suggested test**: Add a regression test for the exact edge the formatter handles — a literal Unicode NEL (`U+0085`) inside a string. The `format!("\\u{:04x}", other as u32)` path needs to confirm it emits `\u0085` (4-digit hex), not `\u{85}` (Rust-style brace form) or a raw byte.
- **Why**: The `chars()` iterator never produces surrogates, so the 4-digit `\uXXXX` form is always valid JSON — but the test pin is missing.

### [Suggested Test] `route_observe::price_milli_per_mtok` against non-finite rate cards
- **Location**: `src/providers/route_observe.rs:135-141`
- **Suggested test**: With `crate::pricing::rate_card` mocked to return `Some(RateCard { input_per_mtok: Some(f64::INFINITY), .. })`, verify `price_milli_per_mtok` either returns `None` (clamping refuse) or documents its saturation behaviour. Even a comment test is worth it: it pins the future contract.

### [Suggested Test] `metering::enforce_spend_ceiling` denial-path cost stays zero
- **Location**: `src/providers/metering.rs:127-140`
- **Suggested test**: After denying, assert that `record_usage` was NOT called (the inner `process()` future should never have been awaited, per the inline doc). The existing `denied_verdict_returns_a_provider_error_before_the_inner_call_runs` test covers the future-await half; add a parallel assertion that no `LoopTraceEvent::ProviderUsage` reaches the trace sink on a denied principal.

### [Suggested Test] `capability_gate::from_request` token estimate excludes tool-call blocks
- **Location**: `src/providers/capability_gate.rs:104-114`
- **Suggested test**: Construct a request with one `ContentBlock::ToolCall` carrying a 4000-character JSON `arguments` field. Assert that `input_tokens` is the estimate *over prose only*, not the over-counted current value. The current behaviour over-counts; pin whatever the chosen contract is.

### [Suggested Test] `route_policy::balance_group` with empty group
- **Location**: `src/providers/route_policy.rs:404-419`
- **Suggested test**: `balance_group(vec![], LoadBalanceStrategy::RoundRobin, 0, ...)` should return `vec![]` (the early-return `if group.len() <= 1` covers this). The function is currently only exercised with `len >= 2` test inputs.

---

## Summary

| Level | Count |
|-------|-------|
| Critical | 0 |
| Warning | 4 |
| Suggested Test | 5 |

## Prior partial findings — disposition

| Prior finding | Status |
|---------------|--------|
| `route_policy.rs:422` empty-group modulo | **Verified safe** — guarded by `len() <= 1` early return |
| `route_policy.rs:189` cast | **Verified safe** — `permille.min(u64::from(u16::MAX))` clamps before cast |
| `retry.rs:44` f64 cast | **Verified safe** — saturating semantics, jitter helper is the only consumer |
| `route_observe.rs:141` cast | **New warning** — saturating cast is well-defined in current Rust, but no defence-in-depth for non-finite rate cards |
| `route_handle.rs:382` `unreachable!` | **Verified test-only** — inside `#[cfg(test)] fn concurrent_store_is_never_observed_torn` |
| All `panic!` in `src/providers/*.rs` | **Verified test-only** — every hit is inside `#[cfg(test)]` |

## Notes

**What's already good** — the module follows project guidelines exceptionally well:

- The `salvage_malformed_args` / `merge_usage` / `ToolCallArgsComplete` / `parse_json_tool_call` quartet in `delta.rs` is a textbook example of "fail open, never silently amputate arguments" — every emission-defect class (control chars, trailing commas, invalid escapes, leading-arg-before-id, authoritative-vs-streamed conflict, truncated JSON) has its own named test. The non-additive repair contract (never close unbalanced braces) is correctly enforced and pinned by `salvage_never_completes_truncated_json`.
- The `llm_retry.rs` shared `is_transient_overload` + `ACCOUNT_SCOPE_PATTERNS` + `RATE_LIMIT_TEXT_PATTERNS` constants are exactly the right shape — three classifiers that used to re-derive narrow lists and silently disagree now share one source.
- `http_provider.rs` TTFB watchdog + `wrap_idle_timeout` + stale-encrypted-reasoning recovery + truncated-tool-call surfacing form a tight layered safety net; every layer's typed error is one the classifier already knows how to handle.
- `route_policy.rs` ordering layers (tier gate → pin promote → sideline gate → strategy balance → cross-tier append) are exactly five rules in one place with stable partitions; the `(t, CandidateAction)` tuple carries the verifier down the chain rather than re-deriving it.
- `metering.rs` price-source decoupled from `name()` ("every production inner is `FailoverProvider`, so `name()` is the literal `"failover"`") is the kind of trap that would have shipped silently if anyone had not written the comment.
- `load_stats.rs` lock-free DashMap + CAS-roll epoch window is the right shape; the `now_secs()` / `now_min()` monotonic helpers are documented with their wall-clock-vs-monotonic justification.
- `health.rs::classify_provider_error_message` correctly delegates to `llm_retry::has_status_code` rather than re-deriving a substring check (the same trap the rest of the codebase fixed).
- All production `lock()` calls (route_witness, session_moa_handle, session_model_handle) use `.unwrap_or_else(|e| e.into_inner())`. Test files (`failover/tests.rs`, `route_handle.rs`, `metering.rs::tests`) use `std::sync::Mutex::lock().unwrap()` directly — OK per project policy.

**Things deliberately NOT changed** (out of scope or already pinned):

- The `route_observe.rs:141` saturating-cast concern — current Rust makes it well-defined; the suggestion is a defensive clamp, not a fix for a present bug.
- The `catalog::supports` "missing metadata = Chat only" rule — documented behaviour, every production chat preset registers metadata, fail-open by construction.
- The `capability_gate::from_request` overcount — pessimistic by design, fail-open at the list level.
- The `metering.rs` wall-clock-vs-monotonic choice — wall-clock is correct for period boundaries; the suggestion is a comment, not a code change.
- The `provider_vault_key` format string in `probe.rs` — single definition, single test, used by both gateway handlers and the doctor check. Not a finding.

**No code changes were applied** — this is a static review only.