# Classification & Routing Matrix: PII

**Target Subsystem**: `src/pii` (12 files, ~2,920 LoC)  
**Reviewer Role**: Jev Tier Static Reviewer  
**Doctrine**: Occam's Razor (`/rust-occams-razor`: Simplicity is the removal of unnecessary concepts, not merely line count reduction)  
**Brief**: `docs/superpowers/runs/2026-11-11-occams-orchestrator-pii-projects/BRIEF_COMMON.md`  

---

## Executive Summary

The `src/pii` subsystem implements gateway-level PII detection, allowlisting, platform overrides, and redaction masking before outbound text reaches LLM providers.
An audit of all 12 assigned files confirms that core domain invariants—such as priority-based overlap deduplication (`Block > Warn`, followed by `PiiSeverity`), trimming of wider losers to prevent cleartext leakage, strict Chinese ID card ISO 7064 validation, Luhn checksum gating, and `FailsOpen` singleton capability slot semantics—are sound.

However, static analysis reveals significant Occam's Razor opportunities on hot execution paths:
1. **Redundant Allocations in Validation Loops**: `IdCardRule` allocates two intermediate heap `String`s (`digits_only` and `normalized`) for every 18-character regex candidate; `BankCardRule::luhn_check` allocates a `Vec<u32>` heap buffer on every card match; `PiiEngine::is_provider_excluded` unconditionally allocates an owned `String` via `to_ascii_lowercase()` even when no provider exclusions are configured.
2. **Double String Allocations on Clean Matches in Overlap Dedup**: In `dedup_overlapping`, when a candidate match does not intersect any existing kept span (>99% of cases), it discards the original `PiiMatch` and allocates 3 brand new heap `String`s (`rule_name`, `placeholder`, `matched_text`), incurring gratuitous allocation and deallocation overhead.
3. **Unnecessary Reference Counting (`Arc`) in Static Allowlist**: `PiiAllowlist` stores `Arc<Vec<Regex>>` inside `OnceLock` and clones the `Arc` on every `Default::default()`, despite `SYSTEM_EMAIL_PATTERNS` having a `'static` lifetime (in contrast to `test_phones` and `local_ips` which already borrow `&'static HashSet<&'static str>`).
4. **Inefficient $O(N \log N)$ Sorting of Pre-Sorted Vectors**: `filter_with_config` calls `sorted.sort_by_key(|x| Reverse(x.start))` on the output of `dedup_overlapping`, even though `dedup_overlapping` inserts elements via `partition_point` by `start` and is already strictly sorted in ascending order.
5. **Code Duplication**: `PiiEngine::new` and `PiiEngine::reload` duplicate the construction of `custom_rule_actions` and `excluded_providers`.

A total of **18 findings** were identified: **10 FIX-NOW** and **8 REJECT** (justified retentions to preserve public API stability, defensive boundary checks, and architectural census invariants).

### Routing Metrics Summary

| Taxonomy Category | Candidates Found | FIX-NOW | REJECT |
|---|:---:|:---:|:---:|
| **REDUNDANT-ALLOCATION** | 8 | 5 | 3 |
| **CONTROL-FLOW / INDIRECTION** | 4 | 3 | 1 |
| **DEAD-CODE / ACCIDENTAL-COMPLEXITY** | 3 | 2 | 1 |
| **PANIC-RISK / DEFENSIVE-CHECK** | 3 | 0 | 3 |
| **TOTAL** | **18** | **10** | **8** |

---

## Classification & Routing Matrix

| ID | Location | Taxonomy | Smell Description | Route | Action |
|:---|:---|:---|:---|:---|:---|
| **PII-01** | `src/pii/allowlist.rs:7-34, 79, 88` | `REDUNDANT-ALLOCATION` | `static SYSTEM_EMAIL_PATTERNS: OnceLock<Arc<Vec<Regex>>>` wraps static patterns in `Arc` and calls `Arc::clone` on every `PiiAllowlist::default()`. Because static data lives for `'static`, `Arc` is redundant indirection. | **FIX-NOW** | Change to `OnceLock<Vec<Regex>>`, borrow as `&'static [Regex]`, and store `&'static [Regex]` in `PiiAllowlist`. Eliminate `Arc` allocation and atomic bumping. |
| **PII-02** | `src/pii/engine.rs:233-236` | `REDUNDANT-ALLOCATION` | `is_provider_excluded` calls `provider_name.to_ascii_lowercase()` unconditionally on every outbound request, heap-allocating a `String` even when `self.excluded_providers` is empty (the default configuration). | **FIX-NOW** | Short-circuit `if self.excluded_providers.is_empty() { return false; }`. Check `contains(provider_name)` first or match case-insensitively without allocation. |
| **PII-03** | `src/pii/engine.rs:583-593` | `REDUNDANT-ALLOCATION` | In `dedup_overlapping`, when a match `m` has no overlaps (`parts.len() == 1 && parts[0] == (m.start, m.end)`), it re-allocates 3 new strings (`m.rule_name.clone()`, `text.get(...).to_string()`, `m.placeholder.clone()`) and drops `m`. | **FIX-NOW** | When `parts.len() == 1 && parts[0] == (m.start, m.end)`, move `m` directly into `kept.insert(at, m)`, saving 3 heap string allocations and 3 deallocations per clean match. |
| **PII-04** | `src/pii/rules/bank_card.rs:32-58` | `REDUNDANT-ALLOCATION` | `BankCardRule::luhn_check` executes `let digits: Vec<u32> = number.chars().filter_map(|c| c.to_digit(10)).collect();`, allocating a heap `Vec` for every 13-19 digit candidate string. | **FIX-NOW** | Compute Luhn checksum in a single reverse iterator pass `number.bytes().filter(|b| b.is_ascii_digit()).map(...).rev().enumerate()` on the stack with zero heap allocation. |
| **PII-05** | `src/pii/rules/id_card.rs:209-216` | `REDUNDANT-ALLOCATION` | `IdCardRule::detect` allocates two intermediate heap `String`s (`digits_only` via `.collect()` and `normalized` via `.to_uppercase()`) for every 18-char candidate. | **FIX-NOW** | Filter and uppercase ASCII bytes directly into a stack buffer `[u8; 18]`. If count != 18, immediately skip. Use `std::str::from_utf8(&buf)` for zero-allocation validation. |
| **PII-06** | `src/pii/rules/id_card.rs:120-123` | `REDUNDANT-ALLOCATION` | `is_valid_checksum` calls `id.chars().last()` after verifying `id.len() == 18`, decoding all 18 characters across UTF-8 to find the last character. | **FIX-NOW** | Access `id.as_bytes()[17]` directly in $O(1)$ and compare byte value against `b"10X98765432"`. |
| **PII-07** | `src/pii/engine.rs:416-417` | `CONTROL-FLOW` | `dedup_overlapping` inserts elements with `partition_point` by `start`, guaranteeing ascending sort order. `filter_with_config` then calls `sorted.sort_by_key(|x| std::cmp::Reverse(x.start))`, running an $O(N \log N)$ sort on an already-sorted vector. | **FIX-NOW** | Replace `sorted.sort_by_key(...)` with `sorted.reverse()`, which runs in $O(N)$ with zero comparisons. |
| **PII-08** | `src/pii/engine.rs:280-288` | `CONTROL-FLOW` | `is_platform_excluded` uses 4 levels of nested `if let` blocks (`if let Some(p) ... if let Some(policy) ... if let Some(ref excluded) ...`). | **FIX-NOW** | Flatten with `platform.and_then(|p| self.lookup_platform_policy(p))` and guard clause. |
| **PII-09** | `src/pii/rules/id_card.rs:67-78` | `CONTROL-FLOW` | `is_valid_date` uses 3 separate verbose `match ... { Some(x) => x, None => return false }` blocks for year, month, and day. | **FIX-NOW** | Flatten into `let Some(...) = ... else { return false; };` guard clauses. |
| **PII-10** | `src/pii/engine.rs:158-167, 211-220` | `ACCIDENTAL-COMPLEXITY` | `PiiEngine::new` and `PiiEngine::reload` duplicate the exact table generation logic for `custom_rule_actions` and `excluded_providers`. | **FIX-NOW** | Extract shared private helper `build_lookup_tables(config: &PrivacyConfig) -> (HashMap<String, PiiAction>, HashSet<String>)`. |
| **PII-11** | `src/pii/rules/custom.rs:50` | `CONTROL-FLOW` | `CustomRegexRule::detect` evaluates `self.config.severity.into()` inside the regex match loop on every single match. | **FIX-NOW** | Hoist `let severity = self.config.severity.into();` outside the loop. |
| **PII-12** | `src/pii/engine.rs:580` | `PANIC-RISK` | In `dedup_overlapping`, the loop checks `if s > e { continue; }`. An empty part `s == e` would not be skipped, emitting a 0-width match that fails offset checks in `filter_with_config`. | **FIX-NOW** | Tighten guard to `if s >= e { continue; }`. |
| **PII-13** | `src/pii/mod.rs:27` | `DEAD-CODE` | Re-exports `pub use crate::config::{PiiAction, PlatformPiiPolicy, PrivacyConfig};` look like redundant alias re-exports. | **REJECT** | **Do NOT remove**. External integration tests (`tests/security_integration.rs`) and `--all-targets` depend on these re-exports from `crate::pii`. |
| **PII-14** | `src/pii/engine.rs:58-65` | `REDUNDANT-ALLOCATION` | `PiiMatch` fields (`rule_name`, `matched_text`, `placeholder`) are owned `String`s. Refactoring to `&str` or `Cow<'a, str>` would save heap allocations in detection rules. | **REJECT** | **API Contract Boundary**. `PiiMatch` is a public struct returned by `PiiRule::detect` and consumed across modules (e.g. `src/browser/secret_guard.rs`). Changing lifetimes breaks public API. |
| **PII-15** | `src/pii/engine.rs:70-73` | `REDUNDANT-ALLOCATION` | `FilterResult.text: String` causes `FilterResult::unchanged(text)` to heap-allocate `text.to_string()`. Speculatively changing `FilterResult.text` to `Cow<'a, str>`. | **REJECT** | **API Contract Boundary**. `FilterResult` is the primary return type of `PiiEngine::filter` across daemon and provider boundaries. Must remain owned `String`. |
| **PII-16** | `src/pii/engine.rs:430-434` | `DEFENSIVE-CHECK` | Offset bounds and UTF-8 char boundary validation before `replace_range` (`result.is_char_boundary(start) && result.is_char_boundary(end)`). | **REJECT** | **Essential Crash Safety**. Protects against panics in `String::replace_range` on multi-byte text; records `skipped_count` for security audit. |
| **PII-17** | `src/pii/rules/{bank_card, phone, id_card}.rs` | `ACCIDENTAL-COMPLEXITY` | Decimal context (`is_decimal_context`) and word boundary helpers exist separately across rule files with subtle differences. | **REJECT** | **Precision Invariant**. As documented in `id_card.rs:151-156`, the subtle drift (e.g., scientific notation `e`/`E` in ID card vs bank card) is intentional to prevent false positives. Do not unify. |
| **PII-18** | `src/pii/engine.rs:127-134` | `INVARIANT-TIGHTENING` | `PII_ENGINE` singleton slot initialization with `MissingSemantics::FailsOpen`. | **REJECT** | **Architectural Constitution**. FailsOpen status is pinned by test `the_engine_slot_pins_its_missing_semantics` and required by Aleph capability census. |

---

## Detailed Findings by Taxonomy

### A. REDUNDANT-ALLOCATION

#### [PII-01] Eliminate Redundant `Arc<Vec<Regex>>` in `PiiAllowlist`
- **Files & Lines**: `src/pii/allowlist.rs:7-34, 79, 88`
- **Current Pattern**:
  ```rust
  static SYSTEM_EMAIL_PATTERNS: OnceLock<Arc<Vec<Regex>>> = OnceLock::new();
  fn system_email_patterns() -> &'static Arc<Vec<Regex>> { ... }
  pub struct PiiAllowlist {
      test_phones: &'static HashSet<&'static str>,
      system_email_patterns: Arc<Vec<Regex>>,
      local_ips: &'static HashSet<&'static str>,
  }
  impl Default for PiiAllowlist {
      fn default() -> Self {
          Self {
              test_phones: test_phones(),
              system_email_patterns: Arc::clone(system_email_patterns()),
              local_ips: local_ips(),
          }
      }
  }
  ```
- **Analysis**: `test_phones` and `local_ips` borrow directly from `static OnceLock<HashSet<&'static str>>` as `&'static HashSet<&'static str>` with zero heap allocation and zero refcounting. `SYSTEM_EMAIL_PATTERNS` wraps its `Vec<Regex>` in `Arc` and calls `Arc::clone` on every `Default::default()`. Because `SYSTEM_EMAIL_PATTERNS` is a static, its lifetime is `'static`. It can be stored as `&'static [Regex]` without `Arc`.
- **Remediation**:
  ```rust
  static SYSTEM_EMAIL_PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();
  fn system_email_patterns() -> &'static [Regex] {
      SYSTEM_EMAIL_PATTERNS.get_or_init(|| vec![ ... ]).as_slice()
  }
  pub struct PiiAllowlist {
      test_phones: &'static HashSet<&'static str>,
      system_email_patterns: &'static [Regex],
      local_ips: &'static HashSet<&'static str>,
  }
  ```
- **Route**: `FIX-NOW`

#### [PII-02] Eliminate Eager `to_ascii_lowercase()` in `is_provider_excluded`
- **Files & Lines**: `src/pii/engine.rs:233-236`
- **Current Pattern**:
  ```rust
  pub fn is_provider_excluded(&self, provider_name: &str) -> bool {
      let lower = provider_name.to_ascii_lowercase();
      self.excluded_providers.contains(&lower)
  }
  ```
- **Analysis**: Evaluated on every outbound LLM call. When `excluded_providers` is empty (the default for nearly all deployments), it still allocates a heap `String` via `to_ascii_lowercase()`.
- **Remediation**:
  ```rust
  pub fn is_provider_excluded(&self, provider_name: &str) -> bool {
      if self.excluded_providers.is_empty() {
          return false;
      }
      if self.excluded_providers.contains(provider_name) {
          return true;
      }
      self.excluded_providers.iter().any(|p| p.eq_ignore_ascii_case(provider_name))
  }
  ```
- **Route**: `FIX-NOW`

#### [PII-03] Eliminate Triple String Allocation on Clean Spans in `dedup_overlapping`
- **Files & Lines**: `src/pii/engine.rs:583-593`
- **Current Pattern**:
  ```rust
  for (s, e) in parts {
      let part = PiiMatch {
          rule_name: m.rule_name.clone(),
          start: s,
          end: e,
          matched_text: text.get(s..e).unwrap_or_default().to_string(),
          severity: m.severity,
          placeholder: m.placeholder.clone(),
      };
      let at = kept.partition_point(|k| k.start < part.start);
      kept.insert(at, part);
  }
  ```
- **Analysis**: For >99% of detections, matches are disjoint and `parts` has exactly 1 element: `(m.start, m.end)`. The code clones `rule_name`, `placeholder`, and re-slices and allocates `matched_text` into a brand new `PiiMatch`, and then drops `m`. This incurs 3 allocations and 3 deallocations per clean match.
- **Remediation**: If `parts.len() == 1 && parts[0] == (m.start, m.end)`, move `m` directly into `kept`.
  ```rust
  if parts.len() == 1 && parts[0] == (m.start, m.end) {
      let at = kept.partition_point(|k| k.start < m.start);
      kept.insert(at, m);
  } else {
      for (s, e) in parts {
          if s >= e { continue; }
          let part = PiiMatch {
              rule_name: m.rule_name.clone(),
              start: s,
              end: e,
              matched_text: text.get(s..e).unwrap_or_default().to_string(),
              severity: m.severity,
              placeholder: m.placeholder.clone(),
          };
          let at = kept.partition_point(|k| k.start < part.start);
          kept.insert(at, part);
      }
  }
  ```
- **Route**: `FIX-NOW`

#### [PII-04] Stack-Based Luhn Check in `BankCardRule`
- **Files & Lines**: `src/pii/rules/bank_card.rs:32-58`
- **Current Pattern**:
  ```rust
  let digits: Vec<u32> = number.chars().filter_map(|c| c.to_digit(10)).collect();
  ```
- **Analysis**: Card numbers are 13–19 digits. Allocating a heap `Vec<u32>` for every candidate match during regex scanning is unnecessary heap churn.
- **Remediation**:
  ```rust
  let mut count = 0usize;
  let mut sum = 0u32;
  for (i, d) in number
      .bytes()
      .filter(|b| b.is_ascii_digit())
      .map(|b| u32::from(b - b'0'))
      .rev()
      .enumerate()
  {
      count += 1;
      let doubled = if i % 2 == 1 { d * 2 } else { d };
      sum += if doubled > 9 { doubled - 9 } else { doubled };
  }
  if count < 13 {
      return false;
  }
  sum.is_multiple_of(10)
  ```
- **Route**: `FIX-NOW`

#### [PII-05] Stack-Buffered Normalization in `IdCardRule::detect`
- **Files & Lines**: `src/pii/rules/id_card.rs:209-216`
- **Current Pattern**:
  ```rust
  let digits_only: String = matched
      .chars()
      .filter(|c| !c.is_whitespace() && *c != '-')
      .collect();
  let normalized = digits_only.to_uppercase();
  if !Self::is_valid_id_card(&normalized) {
      continue;
  }
  ```
- **Analysis**: Two heap `String` allocations per candidate match. A valid Chinese national ID card is strictly 18 ASCII characters.
- **Remediation**:
  ```rust
  let mut buf = [0u8; 18];
  let mut len = 0usize;
  for &b in matched.as_bytes() {
      if !b.is_ascii_whitespace() && b != b'-' {
          if len < 18 {
              buf[len] = b.to_ascii_uppercase();
              len += 1;
          } else {
              len += 1;
              break;
          }
      }
  }
  if len != 18 {
      continue;
  }
  let Ok(normalized) = std::str::from_utf8(&buf) else { continue };
  if !Self::is_valid_id_card(normalized) {
      continue;
  }
  ```
- **Route**: `FIX-NOW`

#### [PII-06] Byte-Indexed Check Code in `is_valid_checksum`
- **Files & Lines**: `src/pii/rules/id_card.rs:120-123`
- **Current Pattern**:
  ```rust
  let last = match id.chars().last() {
      Some(c) => c.to_ascii_uppercase(),
      None => return false,
  };
  last == expected
  ```
- **Analysis**: After `id.len() != 18` is checked, `id.chars().last()` traverses all 18 characters. `bytes[17]` is already available in $O(1)$.
- **Remediation**: Compare `bytes[17].to_ascii_uppercase()` directly against `CHECK_CODES[(sum % 11) as usize] as u8`.
- **Route**: `FIX-NOW`

---

### B. CONTROL-FLOW / INDIRECTION

#### [PII-07] Replace $O(N \log N)$ `sort_by_key` with $O(N)$ Reversal in `filter_with_config`
- **Files & Lines**: `src/pii/engine.rs:416-417`
- **Current Pattern**:
  ```rust
  let deduped = dedup_overlapping(text, all_matches);
  let mut sorted = deduped;
  sorted.sort_by_key(|x| std::cmp::Reverse(x.start));
  ```
- **Analysis**: In `dedup_overlapping`, every span is inserted at `let at = kept.partition_point(|k| k.start < part.start); kept.insert(at, part);`. As a result, `deduped` is already monotonically sorted by `start` ascending. Running `sort_by_key` executes unnecessary comparison logic.
- **Remediation**: Replace with `sorted.reverse();`.
- **Route**: `FIX-NOW`

#### [PII-08] Flatten Quadruple-Nested `if let` in `is_platform_excluded`
- **Files & Lines**: `src/pii/engine.rs:280-288`
- **Current Pattern**:
  ```rust
  if let Some(p) = platform {
      if let Some(policy) = self.lookup_platform_policy(p) {
          if let Some(ref excluded) = policy.exclude_providers {
              if excluded.iter().any(|e| e.eq_ignore_ascii_case(provider)) {
                  return true;
              }
          }
      }
  }
  ```
- **Remediation**: Flatten with `if let Some(policy) = platform.and_then(|p| self.lookup_platform_policy(p))`.
- **Route**: `FIX-NOW`

#### [PII-09] Flatten `is_valid_date` Match Blocks
- **Files & Lines**: `src/pii/rules/id_card.rs:67-78`
- **Current Pattern**: 3 successive verbose `match id.get(...).and_then(...) { Some(x) => x, None => return false }` blocks.
- **Remediation**: Replace with `let Some(year) = id.get(6..10).and_then(|s| s.parse::<u32>().ok()) else { return false; };` (and same for month, day).
- **Route**: `FIX-NOW`

---

### C. DEAD-CODE & ACCIDENTAL-COMPLEXITY

#### [PII-10] Deduplicate Lookup Table Generation in `new` and `reload`
- **Files & Lines**: `src/pii/engine.rs:158-167, 211-220`
- **Analysis**: Both `PiiEngine::new` and `PiiEngine::reload` duplicate the construction of `custom_rule_actions` and `excluded_providers`.
- **Remediation**: Extract private helper `fn build_lookup_tables(config: &PrivacyConfig) -> (HashMap<String, PiiAction>, HashSet<String>)`.
- **Route**: `FIX-NOW`

#### [PII-11] Hoist Severity Conversion in `CustomRegexRule::detect`
- **Files & Lines**: `src/pii/rules/custom.rs:50`
- **Analysis**: `self.config.severity.into()` is repeatedly evaluated inside `find_iter` on every match.
- **Remediation**: Hoist `let severity = self.config.severity.into();` outside the loop.
- **Route**: `FIX-NOW`

#### [PII-12] Tighten Span Check in `dedup_overlapping`
- **Files & Lines**: `src/pii/engine.rs:580`
- **Analysis**: `if s > e { continue; }` allows a 0-width span (`s == e`) to pass through.
- **Remediation**: Change to `if s >= e { continue; }`.
- **Route**: `FIX-NOW`

---

### D. REJECTED / INVARIANT RETENTION CANDIDATES

#### [PII-13] Retain Public Re-exports in `src/pii/mod.rs`
- **Files & Lines**: `src/pii/mod.rs:27`
- **Analysis**: `pub use crate::config::{PiiAction, PlatformPiiPolicy, PrivacyConfig};` is consumed by integration test targets (`tests/security_integration.rs`). Removing it breaks `--all-targets`.
- **Route**: `REJECT` (Mandatory retention)

#### [PII-14] Retain Public Struct Fields in `PiiMatch`
- **Files & Lines**: `src/pii/engine.rs:58-65`
- **Analysis**: `PiiMatch` uses owned `String` fields. Changing to borrowed `&'a str` or `Cow<'a, str>` would break the `PiiRule` trait signature and downstream callers (`src/browser/secret_guard.rs`).
- **Route**: `REJECT` (Public contract stability)

#### [PII-15] Retain Public Struct Fields in `FilterResult`
- **Files & Lines**: `src/pii/engine.rs:70-73`
- **Analysis**: `FilterResult.text: String` is part of the public API returned by `PiiEngine::filter`. Must remain `String`.
- **Route**: `REJECT` (Public contract stability)

#### [PII-16] Retain Boundary Defensive Checks in `filter_with_config`
- **Files & Lines**: `src/pii/engine.rs:430-434`
- **Analysis**: Pre-validating UTF-8 char boundaries before `replace_range` prevents panics on malformed input or edge cases.
- **Route**: `REJECT` (Crash safety)

#### [PII-17] Retain Divergent Decimal Context Helpers
- **Files & Lines**: `src/pii/rules/{bank_card, phone, id_card}.rs`
- **Analysis**: Differences in context checks between ID cards, bank cards, and phone numbers are intentional domain distinctions (e.g. JSON scientific float notation `e`/`E`).
- **Route**: `REJECT` (Precision invariant)

#### [PII-18] Retain `FailsOpen` Capability Slot Semantics
- **Files & Lines**: `src/pii/engine.rs:127-134`
- **Analysis**: Pinned by `census::every_slot_pins_its_own_missing_semantics`. Must not be altered.
- **Route**: `REJECT` (Architectural invariant)

---

## Invariant Safety Pre-Flight

1. **Public API & Trait Surface**:
   - `PiiRule::name(&self) -> &str`
   - `PiiRule::severity(&self) -> PiiSeverity`
   - `PiiRule::placeholder(&self) -> &str`
   - `PiiRule::detect(&self, text: &str) -> Vec<PiiMatch>`
   - `PiiEngine::new(config: PrivacyConfig) -> Self`
   - `PiiEngine::init(config: PrivacyConfig)`
   - `PiiEngine::reload(config: PrivacyConfig)`
   - `PiiEngine::global() -> Option<Arc<RwLock<Self>>>`
   - `PiiEngine::filter(&self, text: &str) -> FilterResult`
   - `PiiEngine::filter_with_platform(&self, text: &str, platform: Option<&str>) -> FilterResult`
   - `PiiEngine::is_provider_excluded(&self, provider_name: &str) -> bool`
   - `PiiEngine::is_platform_excluded(&self, platform: Option<&str>, provider: &str) -> bool`
   - All trait methods, public struct definitions (`PiiMatch`, `FilterResult`, `PiiSeverity`), and `src/pii/mod.rs` re-exports remain 100% identical.

2. **Detection & Masking Precision**:
   - Luhn checksum computation produces identical boolean results for all card numbers $\ge 13$ digits.
   - Chinese ID card validation continues to verify region codes against `VALID_REGIONS`, leap years and days in `is_valid_date`, and ISO 7064 MOD 11-2 check characters.
   - Timestamp and UUID context guards in `PhoneRule` remain intact.
   - Ssh key header and footer label pairing and base64 body truncation detection remain intact.

3. **Overlap Resolution & Redaction Behavior**:
   - `Block` actions always win over `Warn` actions in overlapping spans.
   - Higher severity wins within the same action.
   - Losers overlapping narrower winners are trimmed to uncovered parts, ensuring no cleartext leaks.
   - `sorted.reverse()` preserves exact back-to-front replacement order required by `replace_range`.

4. **Concurrency & Thread Safety**:
   - Regex instances remain compiled once and accessed via static `OnceLock`.
   - `PiiAllowlist` operates on `&'static` references without data races or locks.
