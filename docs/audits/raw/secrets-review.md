# src/secrets review (raw agent output)

## Summary
- Files scanned: src/secrets/{mod,types,crypto,injection,leak_detector,placeholder,vendor_patterns,vault,vault_resolver,virtual_key_resolver,provider/mod,provider/onepassword}.rs
- Critical: 1, Important: 5, Minor: 6
- Health: yellow

## Strengths
- AES-256-GCM with per-entry HKDF-SHA256 derivation; Zeroizing<[u8;32]>
- DecryptedSecret redaction: Debug and Display print [REDACTED]; Clone/Copy intentionally not derived
- Vault crash safety: open_or_backup with process-local monotonic counter
- Placeholder parser rejects empty names, unbalanced {{, disallowed characters
- Bidirectional leak detector uses SipHasher key over (hash, length) fingerprints
- \b left-anchor on sk-* patterns fixes musk- / task-<uuid> false-positive

## Critical findings

### C-1 `SecretError::NotFound` and `InvalidPlaceholder` Display impls echo user-controlled names verbatim — vault-namespace enumeration vector
- File: src/secrets/types.rs:78-96
- Problem: SecretError::NotFound(String) formats as `Secret '{name}' not found`. Any caller that surfaces Display/to_string of these variants exposes the requested secret name to the model, logs, or UI. The caller in src/guardrails/pii_secrets.rs:107-118 strips the name — proving the leak path is real.
- Why it matters: An attacker iterating `{{secret:NAME}}` placeholders can distinguish vault hits from misses and enumerate the operator's namespace.
- Suggested fix: Make Display for NotFound and InvalidPlaceholder print only "secret not found" / "invalid placeholder". Add a name() accessor or RedactedName newtype for callers that need it.

## Important findings

### I-1 Plaintext retained as plain String during vault re-encryption
- File: src/gateway/security/shared_token.rs:303-321 (in reset_token)
- Problem: `let mut plaintext_entries: Vec<(String, String, EncryptedEntry)> = ...` holds decrypted secrets in heap Strings. Not wiped on drop.
- Suggested fix: Hold decrypted material in secrecy::SecretString or Zeroizing<String>.

### I-2 Leak-detector length/hash LRU caches are independent
- File: src/secrets/leak_detector.rs:269-290 and 292-318 (register_injected)
- Problem: injected_hashes and injected_lens are two independent LruCache each capped at 1024. Under churn, secret A's (hash) can be evicted while its len survives.
- Suggested fix: Track (hash, len) as a single key in one LruCache.

### I-3 `OnePasswordProvider` is a stub — `SecretProvider` trait has no `get_secret`
- File: src/secrets/provider/mod.rs:24-39 and src/secrets/provider/onepassword.rs
- Problem: SecretProvider exposes only provider_type() and health_check(). Never resolves a secret. OP_SERVICE_ACCOUNT_TOKEN leaked into child-process environment. account: Option<String> not Zeroizing.
- Suggested fix: Either delete the 1Password stub or add the missing trait method.

### I-4 `find_all_injected_substrings` is O(n × |lens|) and `lens` is up to 1024
- File: src/secrets/leak_detector.rs:387-446
- Problem: For every registered length the inner loop walks every char_indices of the content. With LRU cap at 1024 lengths and a 100 KB LLM response, ~100M iterations per inbound scan.
- Suggested fix: Use a single rolling hash, then probe each (start, len) against the registered set without re-slicing.

### I-5 `redact_all_matches` invariant enforced only by debug_assert!
- File: src/secrets/leak_detector.rs:472-498
- Problem: Relies on matches being non-overlapping and start-sorted. In release, debug_assert is dropped; content[cursor..start] panics on out-of-order indices or produces silently garbled redaction.
- Suggested fix: Sort matches internally, validate sub-slice via pointer arithmetic that returns Option<usize>, drop debug_assert in favor of runtime check.

## Minor findings
### M-1 validate_secret_name length message says "characters" but checks len() (bytes)
- File: src/secrets/mod.rs:21-27

### M-2 expect("unique names resolved in phase 1") is a panic on programmer-error
- File: src/secrets/injection.rs:130-132

### M-3 SecretError::EncryptionFailed includes raw error from hkdf / aes-gcm
- File: src/secrets/crypto.rs:60-67 and types.rs:87-88

### M-4 INJECTED_HASH_KEY0/1 are pub(crate) const — fingerprint key is not secret
- File: src/secrets/injection.rs:11-12

### M-5 redact_all_matches empty-matches case is unreachable but undocumented
- File: src/secrets/leak_detector.rs:474

### M-6 OnePasswordProvider::classify_error lowercases full stderr into a new String
- File: src/secrets/provider/onepassword.rs:67-95

## Cross-cutting observations
- Zeroization is mostly honoured but inconsistent across boundaries
- SecretError is a leaky Display contract
- Two catalogs problem is well-controlled
- Detectors carry Send + Sync correctly but caches not Arc'd
- bincode::config() deprecation suppression acceptable

## Assessment
**Verdict:** yellow
**One-sentence summary:** Strong crypto core with thoughtful zeroization, but leak-detection error types leak user-controlled names verbatim (vault enumeration vector), re-encryption path silently bypasses zeroize-on-drop invariant, and bidirectional leak detector's independent LRU caches can half-evict registered fingerprints under load.
