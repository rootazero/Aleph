# PII Module Static Audit Report

- **Module**: `src/pii/`
- **Date**: 2026-08-16
- **Files**: 12 (3 module files + 9 rule files); ~2,681 LOC
- **Reviewer**: Code review assistant
- **Scope**: review only — no fixes applied. High-confidence findings (>80%) only.
- **Excluded**: the recent fixes in commits `17c870ea1`, `c59090033`, `56a84247e`, `6fcb50a38`, `3a1663c53`.

## Module Map

| File | LOC | Role | Notes |
|------|-----|------|-------|
| `mod.rs` | 29 | Public re-exports | Re-exports `PiiAction`, `PlatformPiiPolicy`, `PrivacyConfig`, `FilterResult`, `PiiEngine`, `PiiMatch`, `PiiRule`, `PiiSeverity`. |
| `engine.rs` | 861 | Detection pipeline + global singleton | `PII_ENGINE: OnceLock`, `init`/`reload`/`global`, `filter`/`filter_with_platform`. |
| `allowlist.rs` | 92 | Static allowlist | Three static `OnceLock` sets/regexes for phones, IPs, system email patterns. |
| `rules/mod.rs` | 73 | Trait + `build_rules` | `PiiRule` trait, sorted-by-severity-desc registry. |
| `rules/email.rs` | 127 | email detection | Mixed-case regex; case-fold is intentional. |
| `rules/ip_address.rs` | 100 | IPv4 detection | IPv6 explicitly out of scope (documented). |
| `rules/phone.rs` | 299 | CN mobile phone | Anti-FP: word boundary, hex, decimal, timestamp. |
| `rules/id_card.rs` | 322 | CN national ID | Region + date + ISO 7064 checksum. |
| `rules/bank_card.rs` | 205 | Credit card with Luhn | Spaced/hyphenated forms supported. |
| `rules/api_key.rs` | 223 | API tokens | 14 prefix families incl. Bearer with RFC 7235 case-fold. |
| `rules/ssh_key.rs` | 228 | PEM/SSH private key | Header/footer label pairing in `detect` (no regex back-ref). |
| `rules/custom.rs` | 122 | User regex rules | Bounded builder (`safe_regex::bounded_builder`). |

## Wiring Summary (consumers of `pii/`)

| Consumer | Surface | Notes |
|----------|---------|-------|
| `bin/aleph-server/commands/start/mod.rs:160` | `PiiEngine::init` | One-shot init at boot. |
| `bin/aleph-server/.../config.rs:94` | `PiiEngine::reload` | Hot-reload path. |
| `gateway/handlers/search_config/update.rs:197` | `PiiEngine::reload` | Watcher-driven reload. |
| `security/runtime_guard.rs:230-263` | `filter_with_platform`, `is_platform_excluded` | Platform-aware gate. |
| `mcp/redact.rs:19` | `PiiEngine::global().read()` | MCP error redaction. |
| `providers/http_provider.rs:166-180` | `PiiEngine::global().read()`, `is_provider_excluded`, `filter` | Outbound safety at the LLM-call site. |
| `browser/secret_guard.rs:33-36` | `build_rules` (re-uses built-ins) | URL/form/page-content secret scan. |

## Findings

### [High] src/providers/http_provider.rs:170 — Provider-tier filter ignores platform-aware exclusion

**Category:** seam / configuration
**Confidence:** High
**Description:** `HttpProvider::apply_outbound_safety` gates the per-text-block PII filter on `engine.is_provider_excluded(&self.name)` (global config only) and then calls `engine.filter(text)` (also global config). It does NOT consult `is_platform_excluded(platform, provider)` and does NOT receive a platform name from its caller — `HttpProvider` has only `self.name: String` (the provider). As a result, an operator who writes
```toml
[privacy.platform_policies.telegram]
exclude_providers = ["local-llm"]
```
sees the intended bypass at the guardrail layer (`runtime_guard.process_outbound` honours it) but the second-tier filter at `http_provider` then re-applies PII filtering on the unredacted payload — directly contradicting the configuration. The same gap also makes per-platform PII `action` overrides (e.g. `[platform_policies.discord].phone = "warn"`) silently downgraded back to global default at the provider tier.
**Suggested fix:** Propagate the platform name through to `execute` / `apply_outbound_safety` and call `engine.is_platform_excluded(platform, provider)`; use `engine.filter_with_platform(text, platform)` so the platform action overrides also survive. Either that, or remove the second-tier PII filter from `apply_outbound_safety` on the grounds that the guardrail pipeline already covers it.

### [High] src/pii/engine.rs:151 — `PiiEngine::reload` lacks the partial-load summary warning emitted by `new`

**Category:** operations / dead-code
**Confidence:** High
**Description:** `PiiEngine::new` (lines 103-114) computes `configured_custom` vs `loaded_custom` (subtracting the 7 built-ins) and emits a single operator-facing `warn!` summarising "Custom PII rules partially loaded; some patterns failed to compile". `PiiEngine::reload` (lines 151-172) does not — even though it calls the same `build_rules` that is already logging each invalid pattern. Operators hot-reloading a half-broken config see per-rule warns but no top-level "X of Y rules skipped" signal. Asymmetric operational story between boot and reload.
**Suggested fix:** Reuse the same `configured_custom`/`loaded_custom` logic in `reload` (extract it into a small helper, or call it inside both code paths). Or drop it from both — but the asymmetry is more user-hostile than its complete absence.

### [Medium] src/pii/engine.rs:60 — `FilterResult.skipped_count` is computed but never read

**Category:** dead code / forward-looking telemetry
**Confidence:** High
**Description:** `FilterResult.skipped_count: usize` is set inside `filter_with_config` whenever a `Block` action cannot be applied because the matched offsets fall outside the (mutated) text or off a `char_boundary`. The doc-comment explicitly states "the audit pipeline (`runtime_guard`) can use this as a triage signal". Grep across `src/` and `tests/` finds zero non-test readers — `FilterResult::has_detections` covers `blocked_count`/`warned_count` only, and `runtime_guard::apply_filter_result` likewise reads only those two. The field is permanently zero-valued for any consumer; it is also non-zero only in pathological offset-tracking scenarios, where it would be a useful signal — but the signal has no consumer.
**Suggested fix:** Either (a) wire `runtime_guard::apply_filter_result` to escalate `skipped_count > 0` to a Critical audit entry (the original intent) and document the convention in the audit log schema, or (b) drop the field, since the comment-claimed "audit pipeline" consumer does not exist.

### [Medium] src/pii/allowlist.rs — PII allowlist has no direct unit tests

**Category:** test coverage
**Confidence:** High
**Description:** `src/pii/allowlist.rs` ships with **zero** `#[cfg(test)] mod tests`. All coverage of `PiiAllowlist::is_allowed` reaches the function indirectly through engine integration tests (`test_filter_test_phone_allowed`, `test_filter_excluded_provider`, ...). The static sets (`TEST_PHONES`, `LOCAL_IPS`) and the regexes (`SYSTEM_EMAIL_PATTERNS` — pattern widened previously by `56a84247e`/`c59090033` for the case-fold in both directions) are exactly where a regression in pattern semantics would land first. A bug in the case-fold flag or in the test-number list would only be caught by the integration surface, which is broader and slower to navigate during a regression chase.
**Suggested fix:** Add `#[cfg(test)]` to `allowlist.rs` with at minimum: a positive test for each rule-name branch (`phone`, `email`, `ip_address`), a negative test for an unrelated rule (e.g. `api_key` returns false for everything), and a case-fold test for the email branch covering `.example`, `.EXAMPLE`, `noreply@`, `NO-REPLY@`.

### [Medium] src/pii/engine.rs:415 — `dedup_overlapping` is O(n²)

**Category:** performance / unbounded input
**Confidence:** Medium
**Description:** `dedup_overlapping` uses `result.iter().any(|existing| m.start < existing.end && m.end > existing.start)` inside a loop over the input. For typical outbound PII (1–3 matches per message) this is invisible, but the path is on the hot outbound pipeline and the surrounding `filter_with_config` is invoked once per outbound text block. A pathological case — a long string with many adjacent rule hits, e.g. a 1-MB CSV column with many credit-card Luhn-valid numbers — would walk `Vec<PiiMatch>` quadratically. The hot path explicitly takes the lock (`engine.read()`/`write()`), so this scales linearly with contention in the worst case.
**Suggested fix:** Two cheap upgrades that preserve the existing semantics: (a) sort spans by `(start, -end)` first and run a sweep; or (b) bucket by `start/16` and check only the small bucket. Either removes the quadratic worst-case.

### [Medium] src/pii/engine.rs:166 vs runtime_guard.rs:230-263 — Double PII filter with divergent semantics

**Category:** seam / duplication
**Confidence:** High
**Description:** `runtime_guard::process_outbound` calls `filter_with_platform` (platform-aware) and then the request continues to `HttpProvider::apply_outbound_safety` which calls `engine.filter` (global-config, no platform). The two paths use *different* filter functions whose semantics can diverge whenever `[platform_policies.X]` overrides the global config. When `runtime_guard` elected to redact, the http_provider tier is a no-op (PII is gone). When `runtime_guard` elected to *skip* (platform exclusion), the http_provider tier may still redact. This is the same root cause as the High finding above, framed from the "double work + divergent semantics" angle rather than the "operator expectation violated" angle.
**Suggested fix:** Treat `HttpProvider::apply_outbound_safety`'s PII block as redundant: drop it and rely on the guardrail layer. If the second tier is kept for defence-in-depth, gate it on `engine.is_platform_excluded(None, &self.name)` so it does not contradict the guardrail layer's decision.

### [Low] src/pii/engine.rs:209-240 — `effective_config` clones `PrivacyConfig` on three early-return paths

**Category:** quality
**Confidence:** High
**Description:** `effective_config` has three early-return branches — no platform, no platform policy, policy with all-`None` fields — each of which returns `self.config.clone()`. The clone is justified when a mutation follows (the mutated config must be owned), but in the three early paths the clone is identical to the source. With `pii_filtering: true`, seven `PiiAction` defaults, and a few `Vec`s/`HashMap`s in `PrivacyConfig`, each clone is a one-shot allocation per filter call. Hot path, easily avoidable.
**Suggested fix:** Make the function return a thin enum (e.g. `enum EffectiveConfig<'a> { Global(&'a PrivacyConfig), Owned(PrivacyConfig) }`) and call into `filter_with_config` with a match on the variant — or split into `effective_config_owned` (only when needed) and a borrowing version for the identity cases.

### [Low] src/pii/rules/api_key.rs:31 — Bearer-token char class misses `+/=` (base64 padding)

**Category:** logic / detection gap
**Confidence:** High
**Description:** The Bearer branch `(?i:Bearer)\s+[a-zA-Z0-9._\-]{20,}` accepts alphanumerics plus `.`, `_`, `-`. Standard base64 alphabet also includes `+`, `/`, `=`. A `Bearer <token>` containing those characters is not matched by `ApiKeyRule`. The `runtime_guard` layer's `ExecLeakDetector` covers this with a broader `bearer_token` rule (per `src/exec/leak_detector.rs:239-260`), but the PII module's stand-alone capability is documented as "filters outbound messages" — a leaky Bearer in a context where only the PII module ran would slip through.
**Suggested fix:** Widen the bearer token char class to `[a-zA-Z0-9._\-+/=]{20,}` or add an explicit `\+{0,}` branch. Add a regression test asserting that `Bearer abc+def/ghi=` is detected by the PII rule alone.

### [Low] src/pii/engine.rs:124-140 vs 154 — `init` and `reload` build different lookup tables

**Category:** quality / maintenance
**Confidence:** High
**Description:** `PiiEngine::new` and `PiiEngine::reload` both rebuild `custom_rule_actions` and `excluded_providers`, but the constructions are open-coded in two places (not extracted into a helper). If a future field is added (e.g. `effective_providers` or a fast-path `phone_digit_set`), the same code must be edited in three places (`new`, `reload`, and the `effective_config` indexing path). Drift is a likely failure mode.
**Suggested fix:** Extract `fn lookup_tables(config: &PrivacyConfig) -> (HashMap<String, PiiAction>, HashSet<String>)` and call it from both.

## Not-A-Finding Inventory (verified >80% confidence)

These were checked and ruled out so future audits don't re-litigate them:

- **Allowlist wiring**. `PiiAllowlist` is wired into `filter_with_config` (`src/pii/engine.rs:319`) — every non-Off PII match is funneled through `is_allowed`. The static sets are not decorative; they reduce false positives for phones, system-emails, and internal IPs.
- **Custom rule safety**. `CustomRegexRule::new` builds the user-supplied pattern via `crate::security::safe_regex::bounded_builder` which caps compiled size at 1 MiB (see `src/security/safe_regex.rs`). Operator patterns cannot produce a compile-time memory blowup.
- **Rule registration parity**. The 7 built-in rule names ("phone", "id_card", "bank_card", "email", "ip_address", "api_key", "ssh_key") appear both in `build_rules` (`src/pii/rules/mod.rs:43-65`) and in `action_for_rule` (`src/pii/engine.rs:241-256`). Renaming one without the other would break `effective_config` mapping; the test suite exercises both.
- **Backend reference regex panic**. The previously-panicking `\1` back-reference in `ssh_key.rs` was removed (per commit `6fcb50a38`) and replaced with Rust-side label pairing in `SshKeyRule::detect`. The header pattern still uses `\1`-free regex; `the_header_pattern_compiles` test guards against reintroducing it.
- **Severity inversion in dedup**. Recent commit (`3a1663c53`) moved `policy_has_any_override` to module scope; the actual severity/blocks ordering fix is in the comment block at line 326-336 and is exercised by `test_dedup_overlapping_bug_low_start_greater_than_high` and `test_overlap_block_wins_over_higher_severity_warn`.
- **Case-fold in both directions for the case-fold in email/allowlist**. The static patterns use `(?i)` and the engine-side test `test_match_mixed_case_email` guards forward + reverse directions (per `56a84247e`).
- **Concurrent reload on global engine**. Lock discipline (`RwLock`) plus poison-safe recovery (`unwrap_or_else(|e| e.into_inner())`) is consistent throughout; the lock is held only across the brief redactive write — readers are not blocked.
- **Skipping Off-actions in detection**. `filter_with_config` short-circuits with `if *action == PiiAction::Off { continue; }` before `rule.detect(text)`. An Off rule's regex never runs; the per-match loop body never sees Off either.
- **Char-boundary safety on UTF-8 replays**. `filter_with_config` checks `is_char_boundary` on both ends before `replace_range` and increments `skipped_count` on failure (pathological upstream mutation). `phone.rs::is_timestamp_context` snaps its 60-char window to char boundaries before slicing. `id_card.rs` preserves the matched span verbatim (separators and check char included) so the placeholder aligns with the original.
- **`PiiAction::Warn` action does not replace text**. Verified at `engine.rs:391-405`: matches are counted into `warned_count` but the original text is preserved. Caller-visible `Warn` semantics for downstream `runtime_guard` mapping are correct.

## State of the Negative Space

- No fixes applied; the report is read-only.
- No new tests added; the PII module's existing test surface is dense (per-rule + integration in `tests/security_integration.rs`).
- No re-evaluation of commits `17c870ea1`, `c59090033`, `56a84247e`, `6fcb50a38`, `3a1663c53` — these are documented in the brief as already-merging the prior round's fixes.
- Not investigated: cross-layer effects on the `secret_leak_detector` (out of scope); the `leak_detector.rs` PII-adjacent logic in `src/exec/` is listed only as a consumer for context.
