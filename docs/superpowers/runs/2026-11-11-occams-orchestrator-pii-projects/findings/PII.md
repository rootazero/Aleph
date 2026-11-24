# Occam's Razor Remediation: Batch PII (`src/pii`)

## 1. Summary of Changes
- **`src/pii/allowlist.rs`**: Converted `SYSTEM_EMAIL_PATTERNS` from `OnceLock<Arc<Vec<Regex>>>` to `OnceLock<Vec<Regex>>`, borrowing as `&'static [Regex]` in `PiiAllowlist`. Removed atomic reference-counting and heap `Arc` allocations on every `Default::default()`.
- **`src/pii/engine.rs`**:
  - Extracted `build_lookup_tables` helper deduplicating custom rule action mappings and excluded provider lowercase sets between `new()` and `reload()`.
  - Added zero-allocation short-circuit in `is_provider_excluded`: immediately returns `false` if set is empty, and checks `contains(provider_name)` before lowercasing.
  - Flattened 4 nested `if let` blocks in `is_platform_excluded` into clean `let Some(...) = ... else { return false; }` guards.
  - In `dedup_overlapping`, when a candidate match does not overlap any existing span (>99% of cases), moved the `PiiMatch` directly into `kept` instead of reallocating 3 heap strings.
  - Replaced $O(N \log N)$ `sorted.sort_by_key(...)` with $O(N)$ `sorted.reverse()`, taking advantage of the guaranteed ascending order from `partition_point`.
- **`src/pii/rules/bank_card.rs`**: Replaced heap `Vec<u32>` allocation in `luhn_check` with a single reverse iterator pass on the stack.
- **`src/pii/rules/id_card.rs`**:
  - Flattened nested matches in `is_valid_date` using `let Some(...) = ... else`.
  - Replaced UTF-8 decoding in `is_valid_checksum` with direct byte access `(bytes[17] as char).to_ascii_uppercase()`.
  - Replaced dual heap `String` allocations in `IdCardRule::detect` with a stack buffer `[u8; 18]`.
- **`src/pii/rules/custom.rs`**: Hoisted `self.config.severity.into()` outside the regex match loop.

## 2. Invariant Safety Pre-Flight
- Public API signatures and trait exports (`PiiRule`, `PiiEngine`, `PiiMatch`, `FilterResult`, re-exports in `mod.rs`) remain strictly stable.
- Luhn and ISO 7064 MOD 11-2 mathematical invariants are 100% preserved.
- Overlap deduplication semantics (`Block > Warn`, severity order) remain identical.
