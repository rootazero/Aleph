# Logic Review Report
**Module**: src/secrets
**Scope**: ~2604 LOC across 11 files (crypto, injection, leak_detector, mod, placeholder, types, vault, vault_resolver, vendor_patterns, virtual_key_resolver) + provider/{mod,onepassword}.rs
**Date**: 2026-08-28
**Mode**: strict (security-critical)

---

## Findings

### [Critical] WhatsApp vault silently falls back to plaintext when no shared-token manager exists
- **Location**: `src/gateway/interfaces/whatsapp/wa_auth/vault_store.rs:55-89`
- **Trigger condition**: `WaAuthManager::new` (line 36) calls `SharedTokenManager::global_crypto()`; when the daemon has no shared-token manager installed (boot races, test binaries, crash-recovery), `crypto = None`. `save()` then constructs an `EncryptedEntry` whose `nonce = [0u8; 12]` and stores `bincode`-serialized `WaAuthData` directly into `vault.data.entries` (line 78). `load()` (line 95) keys on `entry.nonce != [0u8; 12]` to decide between "decrypt" and "deserialize verbatim" — the latter path silently deserializes the bytes that were stored as plaintext.
- **Expected behavior**: Writing sensitive WhatsApp auth credentials (`creds_blob`, `keys_blob`, `app_state_sync`) to the encrypted vault must FAIL CLOSED when no master key is available. The whole point of the shared-token-gated vault is "no token, no encrypted secret"; the current code does the opposite.
- **Actual behavior**: `creds_blob` (which the `whatsapp-rust` library treats as opaque auth tokens, including Signal session state and signed pre-keys) is written to `secrets.vault` as plaintext bincode whenever the global `SharedTokenManager` slot is empty. An attacker who exfiltrates the vault file (which is `chmod 0600` on Unix but world-readable in `~/.aleph` if the operator ever ran as the wrong user, and on Windows has no ACL protection at all) recovers a working WhatsApp session. The `nonce == 0` discriminator is also trivially forgeable: an attacker who can write to the vault can plant a `nonce = 0` entry next to a real encrypted entry and induce a `load()` that returns *their* plaintext bytes.
- **Suggested fix**:
```rust
pub fn save(&self, data: &WaAuthData) -> Result<(), WaAuthError> {
    let crypto = self.crypto.as_ref().ok_or_else(|| WaAuthError::Serialization(
        "Cannot persist WhatsApp auth: no shared-token manager is installed, \
         refusing to store plaintext credentials in the vault".into(),
    ))?;
    let bytes = bincode::serialize(data)
        .map_err(|e| WaAuthError::Serialization(e.to_string()))?;
    let encrypted = crypto.encrypt(&String::from_utf8_lossy(&bytes))
        .map_err(|e| WaAuthError::Serialization(format!("Encryption failed: {e}")))?;
    let entry = crate::secrets::types::EncryptedEntry {
        ciphertext: encrypted.ciphertext,
        nonce: encrypted.nonce,
        salt: encrypted.salt,
        created_at: chrono::Utc::now().timestamp(),
        updated_at: chrono::Utc::now().timestamp(),
        metadata: crate::secrets::types::EntryMetadata::default(),
    };
    let mut vault = self.vault.lock().unwrap_or_else(|e| e.into_inner());
    vault.set(&self.key(), entry)
        .map_err(|e| WaAuthError::Vault(e.to_string()))
}
```
Also delete the `nonce == [0;12]` branch in `load()` and the `crypto = None` constructor paths — there is no scenario in production where they should succeed. The `with_vault` test helper currently relies on `None`, so update its tests to construct a `SecretsCrypto` instead.

---

### [Critical] Post-substitution outbound scan does NOT check injected fingerprints
- **Location**: `src/security/runtime_guard.rs:317-321` and `src/secrets/leak_detector.rs:313-321`
- **Trigger condition**: `process_outbound` runs `scan_outbound` (NOT `scan_inbound`) on `current_text` AFTER `{{secret:NAME}}` has been substituted with the plaintext secret value. `LeakDetector::scan_outbound` (leak_detector.rs:313) only invokes `scan_patterns` — there is no equivalent of `find_all_injected_substrings` for outbound. A resolved secret whose value is *not* itself a recognizable vendor pattern (e.g. a custom internal API key `Kf83-quiet-brook-91xz` exactly as the test fixture at leak_detector.rs:519 uses, an OAuth bearer token of high entropy, a randomly-generated webhook URL secret) flows through the post-substitution scan with `LeakDecision::Allow`.
- **Expected behavior**: The comment at `runtime_guard.rs:310-314` states "The only thing that would catch an outbound that quotes the resolved secret back is a second scan against the substituted string." That intent is correct; the implementation only delivers half of it. The post-substitution scan must apply the same `(hash, len)` fingerprint check that `scan_inbound` uses — otherwise a tool call whose argument happens to echo back a freshly resolved secret value (curl, jq, dump, log, echo, etc.) reaches the LLM with the plaintext intact. The LLM then has it in its context window, where it can be forwarded to any downstream tool on a future turn.
- **Actual behavior**: A non-pattern-shaped secret value is allowed through to the model on the outbound leg. Inbound detection (LLM echoes the same value back) eventually catches it, but by then the secret is already in the model's context window and the model can quote it to any subsequent tool call that doesn't itself use a placeholder.
- **Suggested fix**:
```rust
// in leak_detector.rs, add a symmetric outbound fingerprint check:
pub fn scan_outbound_with_injected(&self, content: &str) -> LeakDecision {
    let (found_labels, redacted) = self.scan_patterns(content);
    if !found_labels.is_empty() {
        return LeakDecision::Block {
            reason: format!("Outbound leak detected: {}", found_labels.join(", ")),
            redacted_content: redacted,
        };
    }
    let matches = self.find_all_injected_substrings(content);
    if !matches.is_empty() {
        return LeakDecision::Block {
            reason: "Outbound content contained a value identical to an \
                     injected secret".to_string(),
            redacted_content: redact_all_matches(content, &matches, REDACTED_INJECTED),
        };
    }
    LeakDecision::Allow
}
```
Then in `runtime_guard.rs::process_outbound` (line ~320), replace the post-substitution `scan_outbound` call with `scan_outbound_with_injected`. Update the test `test_inbound_blocks_echoed_injected_secret` (line 638) to also assert the outbound path — the existing test only exercises the inbound direction because `MockResolver` returns a non-pattern value.

---

### [Critical] `SharedTokenManager::reset_token` loses concurrent writes between read drop and write acquire
- **Location**: `src/gateway/security/shared_token.rs:317-374`
- **Trigger condition**: `reset_token` flow:
  1. Reads vault under `vault.read()` lock, builds `plaintext_entries: Vec<(String, String, EncryptedEntry)>` of every entry.
  2. `drop(vault)` releases the read lock.
  3. `generate_token()` updates `current_token` and stores new HMAC.
  4. `replace_all(new_entries)` re-acquires `vault.write()` and overwrites.
  
  Between steps 2 and 4, any thread calling `store_secret("foo", …)` runs to completion (acquires `vault.write()`, inserts, releases). The new entry is then SILENTLY DISCARDED by step 4 because `new_entries` was built before the new write. The user's data is in memory but not on disk — and if the process is killed before another `save()`, the secret vanishes.
- **Expected behavior**: Re-encryption under a new token must atomically swap the entire entry set. Any `store_secret` that lands between the read-snapshot and the write-swap must either be preserved (re-encrypt under the new token and merge into `new_entries` under the write lock) or fail with a "vault is being re-keyed" error.
- **Actual behavior**: Lost writes, plus a window where the vault is half-re-encrypted if `replace_all` fails on disk I/O after `generate_token()` succeeded — the token was rotated and the old entries cannot be decrypted by anyone holding the old token anymore.
- **Suggested fix**:
```rust
pub fn reset_token(&self) -> Result<String, SharedTokenError> {
    use std::collections::HashMap;

    // Hold the write lock for the WHOLE operation: no concurrent store_secret
    // can race the re-encryption, and replace_all sees the snapshot we built.
    let mut vault = self.vault.write().unwrap_or_else(|e| e.into_inner());
    let old_crypto = self.crypto()?;

    let mut plaintext_entries: Vec<(String, String, EntryMetadata, i64)> = Vec::new();
    for (name, entry) in vault.entries() {
        let decrypted = old_crypto
            .decrypt(&entry.ciphertext, &entry.nonce, &entry.salt)
            .map_err(|e| SharedTokenError::Storage(
                format!("Decrypt failed for '{name}': {e}")
            ))?;
        plaintext_entries.push((
            name.clone(),
            decrypted,                     // owned briefly, dropped at fn return
            entry.metadata.clone(),
            entry.created_at,
        ));
    }

    // Drop the read-side borrow by going through a save that re-encrypts
    // each entry under the new token. Generate the new token while still
    // holding the write lock so no concurrent store can sneak in.
    let new_token = self.generate_token()?;
    let new_crypto = self.crypto()?;

    let mut new_entries = HashMap::with_capacity(plaintext_entries.len());
    let now = chrono::Utc::now().timestamp();
    for (name, plaintext, metadata, created_at) in plaintext_entries {
        let encrypted = new_crypto.encrypt(&plaintext).map_err(|e| {
            SharedTokenError::Storage(format!("Re-encrypt failed for '{name}': {e}"))
        })?;
        new_entries.insert(name, EncryptedEntry {
            ciphertext: encrypted.ciphertext,
            nonce: encrypted.nonce,
            salt: encrypted.salt,
            created_at,
            updated_at: now,
            metadata,
        });
    }
    vault.replace_all(new_entries)
        .map_err(|e| SharedTokenError::Storage(e.to_string()))?;
    Ok(new_token)
}
```
Also: convert `plaintext` to `secrecy::SecretString` so it is zeroized when the Vec drops, instead of leaving a `String` copy on the heap until the function returns.

---

### [Critical] `find_all_injected_substrings` is O(L × N) — DoS vector via inflated LRU length set
- **Location**: `src/secrets/leak_detector.rs:386-450`
- **Trigger condition**: Every call to `scan_inbound` walks every registered injected length `L` and every char position `N` in the content, slicing and re-hashing. With `INJECTED_LRU_CAP = 1024` distinct lengths and a 1 MiB inbound response, this is ~10^9 hash computations per inbound scan. The LRU is per-process and entries are added by `register_injected` on every request that touches a `{{secret:NAME}}` — an attacker who can induce many distinct-length secret resolutions (a tool that resolves `{{secret:KEY_<n>}}` with `n` cycling through 1024 values, or the legitimate resolution of many distinct plugin secrets in a long-lived session) inflates `lens` to the cap. The cost then dominates the request budget.
- **Expected behavior**: A leak detector that runs on every LLM response should be sub-linear in content length and bounded by `O(N + |leaked set|)`, not `O(L × N)`. The fingerprint set can be a `HashSet<(u64, usize)>` lookup per position via a single rolling hash (Rabin-Karp style), which is `O(N)`.
- **Actual behavior**: Memory exhaustion / request-timeout DoS. The detector is also called by `process_inbound` (`runtime_guard.rs:362`) — every tool result returned by an LLM-tool is re-scanned, so a single session that has touched many secrets multiplies the cost on every subsequent turn.
- **Suggested fix**: Use Rabin-Karp / a single rolling SipHash over each position, and probe `self.injected_hashes` only at positions where the rolling hash lands within a registered length. Sketch:
```rust
fn find_all_injected_substrings<'c>(&self, content: &'c str) -> Vec<&'c str> {
    if self.injected_hashes.is_empty() { return Vec::new(); }
    // Pre-compute one window of each length, hash all windows, classify.
    let mut matches = Vec::new();
    let mut lens: Vec<usize> = self.injected_lens.iter().map(|(&l, _)| l).collect();
    lens.sort_unstable();
    for &len in &lens {
        if len > content.len() { continue; }
        let bytes = content.as_bytes();
        // rolling SipHash over each position (or for len==1, char-by-char)
        for start in 0..=bytes.len() - len {
            let window = &content[start..start + len];
            if !content.is_char_boundary(start + len) { continue; }
            let mut h = siphasher::sip::SipHasher::new_with_keys(
                INJECTED_HASH_KEY0, INJECTED_HASH_KEY1,
            );
            window.hash(&mut h);
            if self.injected_hashes.contains(&h.finish()) {
                matches.push((start, start + len));
            }
        }
    }
    // ... existing non-overlap collapse ...
}
```
For high-volume workloads, switch the cache to `dashmap::DashMap<(u64, usize), ()>` or pre-build a `Vec<(u64, usize)>` sorted by hash so each position can binary-search rather than hashing every window.

---

### [Critical] `VirtualKeyResolver` aliases are baked in at boot and never reload on `security_config.update`
- **Location**: `src/bin/aleph-server/commands/start/orchestrator_init.rs:543-555`
- **Trigger condition**: `build_guardrail_registry` constructs `VirtualKeyResolver::new(vault_resolver, config.secrets_config.virtual_keys.clone())` ONCE at startup. The handler at `src/gateway/handlers/security_config/toml_io.rs:550` reads `secrets.virtual_keys` for serialization, and the `security_config.update` RPC writes the same struct back to `config.toml`. But the running `Arc<VirtualKeyResolver>` still holds the *startup* snapshot of the alias map.
- **Expected behavior**: Updating `[secrets_config].virtual_keys` should take effect for subsequent `{{secret:ALIAS}}` resolutions, with the same live-write semantics already implemented for `AgentDefinition.allowed_users` (per SECURITY.md §"agent_update"). A documented reload boundary (the previous batch of live-apply work made this an explicit claim) is the right model here.
- **Actual behavior**: Operator adds an alias `prod_openai_key = "openai-prod-key-2026-08-28"` via the Panel Security page. The new entry is persisted to disk, the operator expects the alias to work, and the next `{{secret:prod_openai_key}}` resolves against the *old* map → `SecretError::NotFound("prod_openai_key")`. The error path at `runtime_guard.rs:193` surfaces this as `GuardrailDecision::Block { class: Unexpected, reason: "secret resolution failed" }`, but the operator has no way to discover that the alias was added correctly to disk yet the resolver still holds the old map.
- **Suggested fix**:
```rust
// virtual_key_resolver.rs — wrap the map in an Arc<RwLock<HashMap>> so callers
// can hot-swap it without rebuilding the resolver:
pub struct VirtualKeyResolver {
    inner: Arc<dyn AsyncSecretResolver>,
    aliases: Arc<RwLock<HashMap<String, String>>>,
}
impl VirtualKeyResolver {
    pub fn replace_aliases(&self, new_map: HashMap<String, String>) {
        *self.aliases.write().unwrap_or_else(|e| e.into_inner()) = new_map;
    }
    async fn resolve(&self, name: &str) -> Result<DecryptedSecret, SecretError> {
        let aliases = self.aliases.read().unwrap_or_else(|e| e.into_inner());
        let resolved = aliases.get(name).map_or(name, String::as_str);
        self.inner.resolve(resolved).await
    }
}
```
Then in `security_config.update` (`toml_io.rs`), after the on-disk write succeeds, call `crate::secrets::VirtualKeyResolver::global().replace_aliases(new_map)`. Mirror the `set_allowed_users` live-apply pattern from SECURITY.md §"agent_update": only claim `allowed_users_applied_live = true` if the live write returned `Ok`.

---

### [Critical] `OnePasswordProvider` is implemented but never wired to actual secret resolution
- **Location**: `src/secrets/provider/onepassword.rs` (whole file) + `src/bin/aleph-server/commands/secret.rs:181-198` (only call site)
- **Trigger condition**: `SecretProvider::health_check` is the ONLY method on the trait (provider/mod.rs:30). The CLI command `secret providers` (commands/secret.rs:179-198) calls `health_check` to print a status table but never stores the provider anywhere a runtime resolver can reach it. `VaultSecretResolver::resolve` (`vault_resolver.rs:24`) only calls `SharedTokenManager::get_secret`, which reads from the encrypted local vault. `async-trait` is on a sync-vault-only code path; 1Password items never reach `render_with_secrets`.
- **Expected behavior**: A provider declared in `config.toml` under `[secret_providers]` with `provider_type = "1password"` and a service-account token env name should have its items resolvable as `{{secret:op://Vault/Item/Field}}` — the trait and CLI both imply this is supported.
- **Actual behavior**: Configuration is parsed, the CLI prints "Ready" or "Needs Auth", the operator concludes 1Password is integrated, but no 1Password item is ever returned by any resolver. An operator who deletes the local-vault copy of an API key expecting `1password` to take over silently breaks every tool call that referenced it. The trait has no `resolve` method, so there is no seam to wire it through even if the integration were attempted.
- **Suggested fix**: Either (preferred) add a `resolve` method to the `SecretProvider` trait and a `OnePasswordResolver` that shells out to `op read op://Vault/Item/Field`, dispatching in `VaultSecretResolver::resolve` when an `op://` prefix is on the requested name; or (conservative) mark `OnePasswordProvider` and the `[secret_providers]` config block as `#[deprecated]` until the integration lands, and document explicitly in `provider/mod.rs` that the trait is currently health-check-only. The current shape — a CLI green-check for a feature that does not work — is the worse-than-missing case from a security-UX standpoint.

---

### [Warning] `VirtualKeyResolver` has no cycle detection or depth limit on alias chains
- **Location**: `src/secrets/virtual_key_resolver.rs:36-38`
- **Trigger condition**: `resolve(name)` translates name → mapped target and delegates to `inner.resolve(mapped_target)`. If `inner` is itself a `VirtualKeyResolver` (composable decorators are natural), a config like `[virtual_keys] a = "b", b = "a"` causes `resolve("a") → inner.resolve("b") → inner.resolve("a")` → infinite recursion → stack overflow → daemon crash. Single-level aliasing is fine (the current wiring puts the alias layer directly above `VaultSecretResolver`), but the API encourages composition.
- **Expected behavior**: Either reject config entries that form a cycle at parse time (cheaper), or track a visited-set in `resolve` and return `SecretError::InvalidPlaceholder` on cycle (defensive).
- **Suggested fix**:
```rust
async fn resolve(&self, name: &str, visited: &mut HashSet<String>) -> Result<DecryptedSecret, SecretError> {
    if !visited.insert(name.to_string()) {
        return Err(SecretError::InvalidPlaceholder(
            format!("virtual key alias cycle detected at '{name}'")
        ));
    }
    let resolved = self.aliases.get(name).map_or(name, String::as_str);
    self.inner.resolve(resolved).await
}
// public wrapper that mints an empty visited set
```
Or, in the config validator at `src/config/types/secrets.rs::SecretsConfig::validate`, walk the alias graph DFS and reject cycles.

---

### [Warning] `VirtualKeyResolver` is NOT applied to MCP / plugin paths
- **Location**: `src/mcp/manager/actor.rs:151-156`, `src/extension/plugin_ops.rs:362-368`, `src/extension/runtime/wasm/secret_resolver.rs:155-180`
- **Trigger condition**: `McpManagerActor::with_secret_resolver` is invoked with a bare `VaultSecretResolver` in `src/bin/aleph-server/commands/start/mod.rs:555`. The plugin runtime `VaultBackedSecretResolver` and the `plugin_settings_for_runtime` path also construct a bare `VaultSecretResolver`. Only `build_guardrail_registry` (`orchestrator_init.rs:543-555`) wraps the resolver in `VirtualKeyResolver` when `virtual_keys` is non-empty.
- **Expected behavior**: An alias declared in `[secrets_config].virtual_keys` should resolve uniformly across all surfaces. Today an operator who aliases `prod_openai_key = "openai-prod"` and references it as `{{secret:prod_openai_key}}` in a plugin setting has it fail silently (`plugin_secrets.rs:91` drops the key); the same alias in a guardrail-routed tool call works.
- **Suggested fix**: Build the `VaultSecretResolver → VirtualKeyResolver` chain ONCE at startup (a single `Arc<dyn AsyncSecretResolver>`) and hand the same `Arc` to every consumer (`actor.with_secret_resolver`, `plugin_settings_for_runtime`, `VaultBackedSecretResolver::for_current_process`). Centralise the wrapping in a `SecretsBootstrap::build_resolver(config)` factory so no consumer can construct a bare `VaultSecretResolver`.

---

### [Warning] `std::sync::Arc` used directly instead of `crate::sync_primitives::Arc`
- **Location**: `src/secrets/vault_resolver.rs:3`, `src/secrets/virtual_key_resolver.rs:10`
- **Trigger condition**: Both files import `use std::sync::Arc;` while the rest of the codebase routes `Arc` through `crate::sync_primitives` (which conditionally substitutes `loom::sync::Arc` under `cfg(loom)`). The `Sync Primitives Import Rule` (AGENTS.md) explicitly forbids this — the import is supposed to flow through the central alias so a loom test run uses the loom-aware variant.
- **Expected behavior**: Import from `crate::sync_primitives::Arc` so `cargo test --features loom` exercises these resolver decorators under loom's model checker and detects races in their composition.
- **Actual behavior**: A loom test for the `VaultSecretResolver → VirtualKeyResolver` stack would silently use the production `Arc` and miss concurrency bugs. No loom test exists for either resolver today (`grep -r "loom_" src/secrets/` returns empty), so the gap is currently theoretical, but every new consumer of these resolvers inherits it.
- **Suggested fix**:
```rust
// vault_resolver.rs:3 — replace
use crate::sync_primitives::Arc;
// virtual_key_resolver.rs:10 — replace
use crate::sync_primitives::Arc;
```
Add `#[cfg(all(test, feature = "loom"))] mod loom_concurrency` per the L2 convention so future races get caught.

---

### [Warning] `as u64` cast on file-size limit for bincode deserialization
- **Location**: `src/secrets/vault.rs:78`
- **Trigger condition**: `bincode::config().limit(MAX_VAULT_BYTES as u64)` where `MAX_VAULT_BYTES = 16 * 1024 * 1024` (line 38). The `as u64` cast from `usize` is widening on 32-bit platforms (where `usize = u32`) — no truncation. On 64-bit it's a no-op. The outer guard `if bytes.len() > MAX_VAULT_BYTES` (line 67) catches a too-large file BEFORE the cast happens. So this is safe today, but is the kind of cast that bites if someone bumps `MAX_VAULT_BYTES` past `u64::MAX` (impossible) or moves the limit above `usize::MAX` on 32-bit (unlikely but feasible with `4 GiB + 1`).
- **Expected behavior**: A `try_into` chain that surfaces a clear error on overflow, with no `as` casts in crypto-adjacent code.
- **Suggested fix**:
```rust
let limit: u64 = MAX_VAULT_BYTES.try_into()
    .map_err(|_| SecretError::Serialization("vault size limit exceeds u64".into()))?;
#[allow(deprecated)]
let data: VaultData = bincode::config()
    .limit(limit)
    .deserialize(&bytes)
    .map_err(|e| SecretError::Serialization(format!("Failed to deserialize vault: {e}")))?;
```

---

### [Warning] `as_ptr() as usize` pointer arithmetic is platform-dependent
- **Location**: `src/secrets/leak_detector.rs:459, 465`
- **Trigger condition**: `redact_all_matches` reconstructs match offsets via `m.as_ptr() as usize - content.as_ptr() as usize`. On 32-bit platforms the difference overflows for very large content (> 4 GiB, not realistic for a single response). On 64-bit it's safe. The function also assumes the matched slice is a sub-slice of `content`; the `debug_assert!` on line 459 verifies the non-overlap invariant but NOT the sub-slice invariant — a caller passing a `matches` slice from a different string would compute an arbitrary offset and corrupt the output.
- **Expected behavior**: Track the offsets as `usize` indices at match time (where the `&content[start..end]` slice is created), not reconstruct them from pointers later. The `find_all_injected_substrings` already computes `(start, end)` indices; `redact_all_matches` should consume them directly.
- **Suggested fix**:
```rust
fn redact_all_matches(content: &str, ranges: &[(usize, usize)], replacement: &str) -> String {
    debug_assert!(ranges.windows(2).all(|w| w[0].1 <= w[1].0), "ranges must be non-overlapping and ordered");
    let mut out = String::with_capacity(content.len());
    let mut cursor = 0usize;
    for &(start, end) in ranges {
        out.push_str(&content[cursor..start]);
        out.push_str(replacement);
        cursor = end;
    }
    out.push_str(&content[cursor..]);
    out
}
// caller becomes:
redact_all_matches(content, &non_overlapping_ranges, REDACTED_INJECTED)
```

---

### [Warning] `Default::default()` for `SecurityGuardConfig` enables ALL paths including audit
- **Location**: `src/security/runtime_guard.rs:96-105`
- **Trigger condition**: `Default::default()` returns `audit_enabled: true`. `RuntimeSecurityGuard::new` (line 109-114) deliberately overrides it to `false` because the receiver is dropped without anyone holding it — every audit entry would be silently discarded. This is correct, but `SecurityGuardConfig { ..Default::default() }` is the spread pattern used in `orchestrator_init.rs:556`. If a future caller constructs `SecurityGuardConfig::default()` directly (bypassing `new`) and hands it to a custom constructor that does NOT override `audit_enabled`, the audit receiver will silently drop events.
- **Expected behavior**: Either (a) make `Default` match `new`'s behavior (`audit_enabled: false`), or (b) make `audit_enabled` a private field with a builder method so the only way to set it `true` is to also receive the returned `Receiver`.
- **Suggested fix**: change `Default::default()` so `audit_enabled` is `false`, OR introduce `SecurityGuardConfig::with_audit() -> (Self, Receiver)` and make `audit_enabled` a private field of the struct.

---

### [Warning] `DEFAULT_BYTE_PATTERNS` lazy initialization order affects which redaction tag wins
- **Location**: `src/secrets/leak_detector.rs:39-83` (SECRET_PATTERN_SOURCES) and `:114-128` (`default_patterns_bytes`)
- **Trigger condition**: The bytes-side scrubber at `default_patterns_bytes` chains `SECRET_PATTERN_SOURCES` THEN `vendor_patterns`. `LEAK_PATTERNS` (the str-side egress detector) lists the legacy high-confidence entries first THEN `VENDOR_SECRET_PATTERNS`. The two detectors do not share an ordering, so an input that matches both the legacy `OpenAI API Key` (`\bsk-…`) and the vendor `Stripe Secret Key` (`sk_live_…`) gets a different label on the str-side vs. byte-side — fine in isolation, but the two redaction tags differ and the audit log distinguishes them.
- **Expected behavior**: Both detectors should use the same source ordering, so a security auditor tracing a redacted token sees the same tag everywhere.
- **Suggested fix**: Drive both `LEAK_PATTERNS` and `DEFAULT_BYTE_PATTERNS` from a single `pub static SECRET_PATTERN_SOURCES` iterator. The `default_patterns_bytes` already does this correctly; `LEAK_PATTERNS` re-states the legacy entries manually — refactor it.

---

### [Warning] `BLOCK_CLASS_SECRETS` is frozen at `["private_key"]` and never re-evaluated
- **Location**: `src/secrets/leak_detector.rs:88-94`
- **Trigger condition**: Per the docstring, this is "deliberately minimal" and "a frozen hard-filter, not a config knob". The docstring cites the same threat model as `sandbox::command_policy`'s hardline floor. But the surface area is much narrower — only PKCS#8 / algorithm-tagged RSA / EC private-key blocks are refused; classic `.env` files containing API tokens, OAuth client secrets in JSON, and SSH private key dumps are merely REDACTED. The model still receives the surrounding context (line numbers, variable names, comments) after redaction, which is enough for an indirect prompt-injection attack to point at the key by name.
- **Expected behavior**: Either widen `BLOCK_CLASS_SECRETS` to include any `*PRIVATE KEY*` block PLUS OAuth `client_secret`/`refresh_token` shapes that never legitimately appear in shell output, OR document the rationale per-pattern in the source.
- **Suggested fix**: At minimum, add `Authorization: Bearer` followed by a long opaque token as block-class. A more conservative expansion: a `[secrets_config].block_class_overrides` (operator-tunable) layered over the frozen default — explicit opt-in, audit-logged when a pattern is added.

---

### [Warning] `reset_token` keeps decrypted secrets in plain `String` rather than `SecretString`
- **Location**: `src/gateway/security/shared_token.rs:336`
- **Trigger condition**: `plaintext_entries: Vec<(String, String, EncryptedEntry)>` — the middle `String` is the decrypted plaintext secret value, sitting in the heap until the function returns. A core dump (or a heap-inspection tool attached to a debugging session) recovers every secret the operator ever stored, because `String::drop` does not zero memory.
- **Expected behavior**: Wrap plaintext secrets in `SecretString` so they are zeroized on drop. The function already takes seconds to run for big vaults, so the zeroize cost is negligible.
- **Suggested fix**:
```rust
use secrecy::SecretString;
let mut plaintext_entries: Vec<(String, SecretString, EncryptedEntry)> = Vec::new();
for (name, entry) in vault.entries() {
    let decrypted = old_crypto.decrypt(...)?;
    plaintext_entries.push((name.clone(), SecretString::from(decrypted), entry.clone()));
}
```
Then re-encrypt by calling `secret.expose_secret()` on the inner value.

---

### [Warning] `resolved_map: HashMap<String, String>` in `runtime_guard.rs::process_outbound` holds plaintext secret values until function returns
- **Location**: `src/security/runtime_guard.rs:182, 197`
- **Trigger condition**: After resolving `{{secret:NAME}}` placeholders, the orchestrator stores the plaintext values in a `HashMap<String, String>` for the duration of `process_outbound`. The function then performs leak scans and substitutes, so the plaintext lives in this HashMap for the entire pipeline. If the function is awaited inside a long-lived task (e.g. a streaming tool call), the HashMap outlives its useful lifetime.
- **Expected behavior**: Either (a) substitute immediately and drop the HashMap, or (b) wrap the values in `SecretString` (small allocation overhead, but bounds the exposure window).
- **Suggested fix**:
```rust
let mut resolved_map: HashMap<String, secrecy::SecretString> = HashMap::new();
// ...
let value = decrypted.expose();
resolved_map.insert(secret_ref.raw.clone(), secrecy::SecretString::from(value.to_string()));
// ... at substitution time:
for (raw, secret) in &resolved_map {
    current_text = current_text.replace(raw.as_str(), secret.expose_secret());
}
```

---

### [Warning] `bytes.len() > MAX_VAULT_BYTES` is the only outer bound; bincode per-`Vec` limits remain
- **Location**: `src/secrets/vault.rs:60-83`
- **Trigger condition**: A crafted vault file at exactly `MAX_VAULT_BYTES` (16 MiB) containing many small entries each holding a 16 MiB ciphertext `Vec<u8>` would fail the outer length check (total ≤ 16 MiB), but bincode's per-`Vec<u8>` length-prefix limit ALSO caps each individual ciphertext at `MAX_VAULT_BYTES`. The combined bound is correct (each Vec ≤ 16 MiB, file ≤ 16 MiB, sum ≤ 16 MiB), but the docstring claim "the previous code did not bound total heap" is only half-true: the deserialized `HashMap<String, EncryptedEntry>` carries every ciphertext in memory simultaneously, so peak heap is bounded by `entries × MAX_VAULT_BYTES_per_entry` — still ≤ 16 MiB, but the description reads as if this were a stronger bound than it is.
- **Expected behavior**: Either tighten the per-`Vec` bincode limit (`MAX_VAULT_BYTES / EXPECTED_MAX_ENTRIES` so the sum is bounded tighter), or document that "total heap ≤ ~17 MiB" honestly in the comment.
- **Suggested fix**: derive the bincode per-`Vec` limit from `MAX_VAULT_BYTES / 1024` (assuming 1024 entries is the realistic ceiling) and add a unit test that fakes a vault file at the limit to assert peak heap.

---

### [Warning] `OnePasswordProvider::health_check` returns `Ok(ProviderStatus::Unavailable)` on `cmd.output()` I/O failure but treats timeout as a clean `Unavailable` rather than auth-required
- **Location**: `src/secrets/provider/onepassword.rs:96-138`
- **Trigger condition**: When `tokio::time::timeout(OP_INVOCATION_TIMEOUT, cmd.output())` returns `Err(_elapsed)` (line 132), the code maps to `ProviderStatus::Unavailable` with a message implying "may be waiting for an interactive prompt". But the same condition can occur if the user actually IS authenticated and `op` is just slow (network blip). The CLI at `commands/secret.rs:198-201` surfaces this as `Unavailable`, never as `Needs Auth`, so an operator who IS signed in cannot distinguish "needs signin" from "slow op"; both look like a timeout.
- **Expected behavior**: On timeout, retry once with a shorter timeout to disambiguate, or return a distinct `ProviderStatus::Slow { expected_ms }` variant.
- **Suggested fix**: Add a `ProviderStatus::Timeout { after: Duration }` variant so the CLI can render "1Password took >5s — check for an interactive prompt" distinctly from "1Password CLI error".

---

### [Warning] `cargo fmt`-rendered `find_all_injected_substrings` recomputes `len` bounds on every inner iteration
- **Location**: `src/secrets/leak_detector.rs:386-450`
- **Trigger condition**: `content.char_indices()` walks the entire string for EACH registered length (line 420). For 1024 distinct lengths and a 1 MiB UTF-8 string, that's 10^9 `is_char_boundary` checks. This compounds the already-critical O(L × N) DoS above.
- **Suggested fix**: Walk char positions once, then for each position test each registered length that fits. The fingerprint lookup is already O(1); the `is_char_boundary` check is per-position only.

---

### [Warning] `placeholder.rs` rejects chars at the iteration boundary; `extract_secret_refs` panics on `&input[cursor..]` if cursor exceeds len
- **Location**: `src/secrets/placeholder.rs:24-60`
- **Trigger condition**: `while let Some(offset) = input[cursor..].find(PREFIX)` — if a future change ever advances `cursor` past `input.len()` (e.g. a malformed `placeholder_end` calculation under a future refactor), this panics. The current code is safe (`placeholder_end ≤ input.len()`), but the safety relies on invariants that are not enforced by types.
- **Expected fix**: replace with `input.get(cursor..).and_then(|s| s.find(PREFIX))` so the panic becomes a `None` and the `while let` exits cleanly.

---

### [Warning] No `loom` or `proptest` test coverage for any file in `src/secrets/`
- **Location**: `find src/secrets -name "loom_*" -o -name "proptest_*"` returns empty
- **Trigger condition**: The module uses `RwLock<SecretVault>` (vault.rs:79), `Mutex<Option<String>>` (shared_token.rs:80), and a non-trivial `AtomicU64` (vault.rs:50). The async `process_outbound` / `process_inbound` in `runtime_guard.rs` is not a secret-module file but it composes every `secrets` component under `tokio::sync::Mutex`. None of these are exercised under loom or proptest today.
- **Expected behavior**: A bare-minimum `loom_concurrency.rs` for the vault-rwlock pair (read/write/read race that produces no torn reads) and a `proptest_decrypt.rs` for `extract_secret_refs`/`render_with_secrets` (fuzz: random UTF-8 + arbitrary placeholders, round-trip-or-error).
- **Suggested fix**: add `#[cfg(all(test, feature = "loom"))] mod loom_concurrency` to `vault.rs` and `proptest_*` test files for the resolver chain.

---

### [Warning] `DecryptedSecret::expose()` returns `&str` borrowed from a `SecretString` heap allocation that survives until `Drop`
- **Location**: `src/secrets/types.rs:24-30`
- **Trigger condition**: Every consumer of `DecryptedSecret` calls `.expose()` to get a `&str`, then may stash the `&str` in a longer-lived structure (a `HashMap<String, String>` as in `runtime_guard.rs:198`, a `serde_json::Value::String` as in `mcp/manager/secret_resolver.rs:42`, a `command_env` as in `extension/plugin_secrets.rs:84`). The borrower outlives the `SecretString` only if the lifetime is sound — most sites use `.to_string()` (which copies to a plain `String` not zeroized on drop). This is the standard secret-handling pattern, but the callers must remember to `.to_string()` rather than holding the `&str`.
- **Expected fix**: a clippy lint / `#[must_use]` wrapper that reminds callers to convert, or a single `into_string(self) -> Zeroizing<String>` method that consumes the `SecretString` and yields a zeroized owned buffer.

---

### [Warning] `validate_secret_name` allows `:` which collides with the placeholder parser's namespace syntax
- **Location**: `src/secrets/mod.rs:42` and `src/secrets/placeholder.rs:46`
- **Trigger condition**: `validate_secret_name` permits `:` in names. The placeholder parser at `placeholder.rs:46` ALSO permits `:` and explicitly tests the namespace case `anthropic:prod-key_1`. So `{{secret:foo:bar}}` is valid — but it cannot be distinguished from `{{secret:foo}}` followed by the literal `bar}}`. The current placeholder parser is greedy on the `}}` suffix (it scans for the first `}}` after the prefix), so `{{secret:foo:bar}}` parses as name `foo:bar`. This works.
- **Risk**: Low today (the parser is consistent). But the `:` allowance means `set("ns:key", …)` and `delete("ns:key")` are valid — a malicious actor who can write the vault could plant `ns:../sensitive_key` and have it accepted. The char set is too permissive for a "secret namespace" — `:` should be a reserved structural character, not a name char.
- **Suggested fix**: tighten `validate_secret_name` to `[A-Za-z0-9._-]` and require the namespace separator to be a different field (e.g. `name.namespace`). Update `placeholder.rs` to match. Document the migration for existing namespaced entries.

---

### [Suggested Test] `SharedTokenManager::reset_token` must preserve concurrent writes
```rust
#[tokio::test]
async fn reset_token_preserves_concurrent_store_secret() {
    let store = Arc::new(SecurityStore::in_memory().unwrap());
    let dir = TempDir::new().unwrap();
    let mgr = Arc::new(SharedTokenManager::new(store, dir.path().join("test.vault")));
    mgr.generate_token().unwrap();
    mgr.store_secret("alpha", "first").unwrap();

    // Spawn a writer that lands between read-drop and write-acquire.
    let mgr2 = mgr.clone();
    let writer = tokio::spawn(async move {
        // Sleep just enough to land in the window.
        tokio::time::sleep(Duration::from_millis(50)).await;
        mgr2.store_secret("beta", "second").unwrap();
    });

    mgr.reset_token().unwrap();
    writer.await.unwrap();

    // Both secrets must still be retrievable.
    assert_eq!(mgr.get_secret("alpha").unwrap().unwrap().expose(), "first");
    assert_eq!(mgr.get_secret("beta").unwrap().unwrap().expose(), "second");
}
```

### [Suggested Test] Outbound post-substitution fingerprint block
```rust
#[tokio::test]
async fn test_outbound_post_substitution_blocks_injected_fingerprint() {
    // Regression for CRITICAL-2 in this audit:
    // a non-pattern-shaped secret value that gets echoed in tool args must be
    // blocked at outbound, not just at inbound.
    let (guard, _rx) = RuntimeSecurityGuard::new_with_audit(SecurityGuardConfig::default());
    struct CustomValueResolver;
    #[async_trait]
    impl AsyncSecretResolver for CustomValueResolver {
        async fn resolve(&self, _: &str) -> Result<DecryptedSecret, SecretError> {
            Ok(DecryptedSecret::new("Kf83-quiet-brook-91xz".to_string()))
        }
    }
    let ctx = SecurityContext::default();
    // The model "echoes" the secret in a follow-up tool call:
    let result = guard.process_outbound(
        "tool_arg=echo-the-value:Kf83-quiet-brook-91xz",
        Some(&CustomValueResolver),
        ctx,
    ).await.unwrap();
    assert!(matches!(result, GuardResult::Blocked { .. }),
        "post-substitution outbound must block injected fingerprints, got {:?}", result);
}
```

### [Suggested Test] Loom test for `find_all_injected_substrings` race
```rust
#[cfg(all(test, feature = "loom"))]
mod loom_concurrency {
    use loom::sync::Arc;
    use loom::thread;
    use crate::secrets::leak_detector::{LeakDetector, InjectedSecret};

    #[test]
    fn injected_registration_race_no_panic() {
        loom::model(|| {
            let det = Arc::new(loom::sync::Mutex::new(LeakDetector::new()));
            let s1 = "secret-value-1".to_string();
            let s2 = "secret-value-2-different-length".to_string();

            let d1 = det.clone();
            let t1 = thread::spawn(move || {
                d1.lock().unwrap().register_injected(&[
                    InjectedSecret::from_value("k1", &s1),
                ]);
            });

            let d2 = det.clone();
            let t2 = thread::spawn(move || {
                d2.lock().unwrap().register_injected(&[
                    InjectedSecret::from_value("k2", &s2),
                ]);
            });

            t1.join().unwrap();
            t2.join().unwrap();

            let d = det.lock().unwrap();
            assert!(d.scan_inbound(&format!("echoing: {s1}")).is_blocked());
            assert!(d.scan_inbound(&format!("echoing: {s2}")).is_blocked());
        });
    }
}
```

### [Suggested Test] `extract_secret_refs` is panic-free for adversarial inputs
```rust
#[test]
fn extract_secret_refs_does_not_panic_on_adversarial_input() {
    use proptest::prelude::*;
    proptest!(|(s in "\\PC{0,4096}")| {
        // Either parses cleanly or returns InvalidPlaceholder — never panics.
        let _ = crate::secrets::placeholder::extract_secret_refs(&s);
    });
}
```

### [Suggested Test] `WaAuthManager::save` refuses plaintext fallback
```rust
#[test]
fn wa_auth_save_fails_when_no_crypto() {
    let dir = TempDir::new().unwrap();
    let vault = SecretVault::open(dir.path().join("test.vault")).unwrap();
    let auth = WaAuthManager::with_vault(vault, "no_crypto_account");
    let data = WaAuthData { creds_blob: vec![1,2,3], keys_blob: vec![], app_state_sync: vec![] };
    assert!(auth.save(&data).is_err(),
        "save() must refuse plaintext fallback when no crypto engine is available");
}
```

### [Suggested Test] `VirtualKeyResolver::replace_aliases` is live
```rust
#[tokio::test]
async fn virtual_key_resolver_replaces_aliases_live() {
    let mut aliases = HashMap::new();
    aliases.insert("openai".to_string(), "prod_openai_key".to_string());
    let resolver = Arc::new(VirtualKeyResolver::new(Arc::new(StubResolver), aliases));

    assert_eq!(resolver.resolve("openai").await.unwrap().expose(), "value-for:prod_openai_key");

    let mut new_aliases = HashMap::new();
    new_aliases.insert("openai".to_string(), "staging_openai_key".to_string());
    resolver.replace_aliases(new_aliases);

    assert_eq!(resolver.resolve("openai").await.unwrap().expose(), "value-for:staging_openai_key");
}
```

---

## Security-Specific Concerns

### Cryptographic Weaknesses

1. **No key versioning / rotation of the master key itself** (vault.rs:43, crypto.rs:34). The `master_key` is whatever `SecretString` was passed in at construction; there is no API to rotate it without re-encrypting every entry. `SharedTokenManager::reset_token` re-encrypts under a *new token*, but the underlying HKDF-SHA256 derives a per-entry key from `master_key`, so the per-entry key is `HKDF(master, salt, "aleph-secrets-v1")`. Rotating `master_key` without rotating the `version` label produces keys that are cryptographically distinct but indistinguishable from old keys on disk — a future code change to the `HKDF_INFO` constant silently invalidates every vault. There is no `format_version` for the KDF domain separator.
2. **HKDF INFO label `"aleph-secrets-v1"` is hard-coded** (crypto.rs:34). A migration to v2 cannot be done by reading a flag — it requires code change. The vault `version` field is the file format, not the KDF parameters.
3. **Per-entry random salt is generated but the per-vault salt is reused** (crypto.rs:64-66). Two entries with the same plaintext + different salts produce different ciphertexts (good), but the entropy bound is the per-entry salt (32 bytes) — fine.
4. **AES-256-GCM nonce uniqueness depends on `rand::rng()`** (crypto.rs:68). Nonce reuse is catastrophic for GCM (key recovery). The code uses `rand::rng()` (thread-local ChaCha-based CSPRNG in `rand` 0.9) which is suitable, but a future contributor swapping in a deterministic RNG for "testability" would silently break this. Consider asserting `nonce != [0u8; 12]` and a counter on the wrapped `SecretsCrypto` to guarantee uniqueness across the process lifetime.
5. **Master key is held as `String` inside `SecretString`** (crypto.rs:42). `SecretString` zeroizes its heap allocation on drop, but the HKDF input `self.master_key.expose_secret().as_bytes()` (line 53) makes a temporary `&[u8]` that the HKDF copies — fine, but the master key never leaves `SecretString` as a zeroized buffer. A future contributor passing the master key to a function expecting `&mut [u8]` for "speed" would create a copy that is never zeroized.
6. **No integrity over `EntryMetadata`** (types.rs:54-64). `description` and `provider` are plaintext fields next to the ciphertext. An attacker who can write to the vault can change `description = "this is not a secret"` to social-engineer an operator into deleting the entry, or change `provider` to match a different entry's name to confuse leak-detector tag selection.
7. **Vault `version` field is not authenticated** (types.rs:74). A future-version vault is rejected (`vault.rs:84-87`), but a downgraded version (e.g. v1 → v0 if v0 were ever defined) would be silently accepted — the version check is a `>` not `!=`. Reject any unknown version.
8. **WhatsApp vault plaintext fallback** (Critical-1 above) — `nonce = [0u8; 12]` is a structural bypass that turns the encrypted vault into a plaintext bincode store.
9. **`VirtualKeyResolver` aliases are not bound to any integrity check** — an attacker who can write to `config.toml` can silently redirect every alias to a vault key they control. No signature/HMAC on the alias map.

### Secret Leak Risks

1. **`resolved_map: HashMap<String, String>` in `runtime_guard.rs::process_outbound`** (line 197) holds plaintext secret values until function return — longer if the function is `.await`-ed in a long task.
2. **`reset_token` plaintext `Vec<(String, String, EncryptedEntry)>`** (shared_token.rs:336) holds every decrypted secret in heap memory until the function returns.
3. **`InjectedSecret::from_value` stores `value_len` in plaintext** (injection.rs:41) — `value_len` is the secret length, which leaks structure to the leak detector's logs but no plaintext. Low risk, but if the hash ever weakens this becomes a side channel.
4. **`http_provider.rs:194` and `:451` instantiate `LeakDetector::new()` per call** (providers/http_provider.rs) — the fresh detector has no `register_injected` history, so injected values that bypass guardrails' process_outbound (e.g. via direct internal API calls that skip the input guardrail) are not caught on the http_provider second pass. The guardrail layer is supposed to be the only gate, but defense in depth implies this is a redundant belt to a missing suspender.
5. **`VaultIo::write` does not sync the parent directory after rename** (utils/vault_io.rs:46 + utils/atomic_io.rs). On a power loss between `rename` and the directory entry sync, the file may be present but the directory entry may point at the old (or no) inode. The vault reads return `Ok(None)` and the user starts a fresh empty vault — destroying all stored secrets on next save. `atomic_write.rs` (separate file) likely handles this; verify the `write_atomic` path. The `open_or_backup` recovery at least moves the bad file aside, but if no entry exists, recovery is moot.
6. **`tracing::debug!` logs secret names verbatim** (onepassword.rs:75) — `lower.contains("not signed in")` then logs the stderr, which may include the requested item name. Already mitigated by the past review (raw stderr goes to debug only, user-facing message is fixed text). Verify the same discipline holds for ALL `tracing` calls in the secrets module.
7. **In-process hex values used as SipHash keys** (injection.rs:13-14) — known to anyone with the binary. An attacker who can compute `siphash` of arbitrary windows can craft a string that has the same hash as a known secret length, bypassing the leak detector. The bypass direction is "false positive" (over-redaction), not "leak", but worth noting.
8. **`Block { reason: … }` in `GuardResult` includes the leaked-secret label** (leak_detector.rs:316) — labels are static ("Anthropic API Key"), not the actual value, so safe. But `secret_injection` errors include the secret NAME in the reason (see CRITICAL-2's discussion in pii_secrets.rs:107-122 — already partially mitigated).
9. **`extract_secret_refs` does not redact on parse failure** (placeholder.rs:33-39) — the error message includes the offending substring, which is the user's own input. Fine.
10. **Plugin settings echo placeholder back to the runtime** if resolution fails (extension/plugin_secrets.rs:91) — drops the key entirely. Correct behavior. But note that a malicious plugin could name a setting `{{secret:ALIAS}}` to detect whether `ALIAS` resolves (oracle); the drop doesn't return a distinct "not found" vs "not allowed" error to the plugin code, but the JSON shape change is observable.

### Auth/Access Control Bypass Risks

1. **`validate_secret_name` allows `:` which is the placeholder namespace separator** (mod.rs:42) — a vault key `ns:../foo` parses but references could be confused with paths. See Warning above.
2. **`open_or_backup` does not check ownership of the vault file** (vault.rs:131-149) — an attacker who can place a file at `~/.aleph/secrets.vault` (e.g. via a symlink attack before `data_dir` is created) controls the on-disk vault content. The `VaultIo` lockfile is created alongside; if the attacker pre-creates both with the right permissions, the daemon happily loads the attacker's vault.
3. **`default_path()` falls back to `secrets.vault` in CWD on `get_config_dir` failure** (vault.rs:294-303) — a CWD-controlled path is a privilege escalation vector when the daemon is started by an unprivileged child of the operator.
4. **`SecretProvider` trait has only `health_check`** (provider/mod.rs:29-32) — there is no actual access-control boundary on provider use. A caller with the trait object can call `health_check` only; the actual vault resolution path (`VaultSecretResolver`) does not consult any "is this caller allowed to read this secret" gate.
5. **`VirtualKeyResolver` does not check `allowed_patterns`** (extension/runtime/wasm/capability_kernel.rs::check_secret_pattern is on the WASM side, not the secret side) — an operator alias `prod_key = "system_master_key"` makes `system_master_key` accessible via `{{secret:prod_key}}` regardless of any per-plugin `[capabilities.secrets] allowed_patterns` configuration. The secret module does not consult capability rules; this is a layering concern (see cross-module).
6. **The audit log captures only `actor_user` for blocked secret leaks** (runtime_guard.rs:142-150) — the `event_type` is `ExecBlocked` or `EnvInjectionDetected`, but the originating RPC method (`agent.run`, `chat.send`, `tools.invoke`) is not recorded. An operator triaging "who triggered this leak" cannot distinguish a CLI invocation from a Panel chat from a Telegram bot from a cron job.
7. **`SharedTokenManager::reset_token` is gated only by `is operator`** (per SECURITY.md §"agent_update") — the RPC handler at `gateway_token::handle_token_rotate` should verify operator role before invoking; this is not verified inside `reset_token` itself. Defense in depth would have `reset_token` assert `caller_role == operator` via a `TurnContext` parameter.

### TOCTOU / Race Condition Risks

1. **`reset_token` vault read → drop → write** (Critical-3 above) — concurrent `store_secret` calls land in the window and are lost.
2. **`store_secret` does not check the entry exists under a write lock** (shared_token.rs:225-242) — `vault.set` is internally `&mut self`, so no inter-thread race inside one `SecretVault`. Safe.
3. **`delete` then `get` race** — both take `&mut self` on the vault, no race.
4. **`open()` then `save()` on disk** — `VaultIo::write` uses `with_file_lock`, so two processes sharing the lock file serialize. But the `open_or_backup` path (vault.rs:131) calls `fs::rename` OUTSIDE the lock — another process can rename the same `.corrupt` suffix within the same second; the `CORRUPT_BACKUP_COUNTER` mitigates this.
5. **`find_all_injected_substrings` walks `self.injected_hashes` and `self.injected_lens`** without a lock (leak_detector.rs:391-449). The runtime_guard wraps both in `tokio::sync::Mutex` so calls serialize, but if a future caller forgets to hold the lock, two concurrent `register_injected` + `scan_inbound` calls can produce torn reads on the LRU.
6. **`VirtualKeyResolver::resolve` reads `self.aliases`** (virtual_key_resolver.rs:36-38) without holding any lock — concurrent alias mutation would race. Today the alias map is immutable after construction; if it becomes mutable (per Critical-5 fix), the read needs a lock.
7. **`SharedTokenManager::current_token`** (shared_token.rs:80) — `Mutex<Option<String>>` is used; every accessor uses `unwrap_or_else(|e| e.into_inner())` correctly.
8. **Plugin settings resolution** (`plugin_settings_for_runtime` → `resolve_settings`) reads the plugin settings JSON snapshot at call time and resolves secrets against the live vault. If `store_secret` happens between snapshot and resolution, the snapshot is stale — but the operator's intent was to read "current" settings, so this is a feature, not a bug. No action needed.
9. **`runtime_guard.rs::process_outbound` re-locks `self.exec_leak_detector` and `self.secret_leak_detector` four times** (line 165, 233, 320, 350) — each lock acquires+releases for a single scan. If two threads enter `process_outbound` concurrently (which they do, one per request), the lock acquisitions serialize. The current `tokio::sync::Mutex` is correct; under loom, this should be exercised.

---

## Provider Registry Audit

| Provider File              | Trait Implemented | Registered in Factory | Wired to Caller                                                  |
|----------------------------|-------------------|----------------------|------------------------------------------------------------------|
| `provider/mod.rs`          | n/a               | n/a                  | Trait definition only — no factory function exists               |
| `provider/onepassword.rs`  | `SecretProvider`  | **No factory**       | CLI `secret providers` (commands/secret.rs:181) — health-check only; never wired to a `SecretResolver` |

**Wiring gap**: the `SecretProvider` trait exposes only `health_check` and `provider_type` — there is no `resolve` method. The OnePasswordProvider therefore cannot serve secrets through `AsyncSecretResolver`, despite configuration and CLI plumbing that implies it can. This is the worst kind of gap: a green CLI indicator for a feature that does not exist at runtime.

---

## Wiring Gaps (this module → outside)

| Item                              | Type            | Status                                                     | Should be used by                                                              |
|-----------------------------------|-----------------|------------------------------------------------------------|--------------------------------------------------------------------------------|
| `extract_secret_refs`             | fn              | Wired                                                     | `runtime_guard.rs`, `injection.rs`, `mcp_config.rs`, `hub/secrets.rs`           |
| `render_with_secrets`             | fn              | Wired                                                     | `mcp/manager/secret_resolver.rs`, `extension/plugin_secrets.rs`                |
| `validate_secret_name`            | fn              | Wired                                                     | `admin_api/secrets.rs`, `handlers/secrets.rs`                                  |
| `VaultSecretResolver`             | struct          | **Partial** — applied at guardrail + plugin + WASM but bare at MCP | every `Arc<dyn AsyncSecretResolver>` consumer should funnel through a single bootstrap |
| `VirtualKeyResolver`              | struct          | Wired at boot in `build_guardrail_registry`; not reloaded on `security_config.update`; NOT applied at MCP/plugin/WASM paths | everywhere `VaultSecretResolver` is constructed today |
| `OnePasswordProvider`             | struct          | **No** — health-check only                                | should either implement `resolve` via `SecretProvider` trait extension, or be marked deprecated |
| `SecretProvider` trait            | trait           | Partial — only `health_check`                              | needs a `resolve(&str) -> Result<DecryptedSecret, _>` method to be useful       |
| `Blocked Secret` audit detail     | AuditEventType  | Wired but minimal                                         | should include the RPC method that triggered the leak                          |
| `BLOCK_CLASS_SECRETS` overrides   | config field    | **No** — frozen hardcoded list                            | operator should be able to add block-class patterns with audit log entry       |
| `INJECTED_LRU_CAP`                | const           | Wired but capped at 1024 without performance bound         | needs a per-call time budget to fail open (log + skip) if exceeded              |
| `decrypted_secret.expose()` lifetime | API            | **No lifetime bound** — callers commonly `.to_string()`     | needs `into_zeroizing(self) -> Zeroizing<String>` consuming method              |

---

## Lock/Cross-Module Concerns

1. **`security/runtime_guard.rs`** is the single chokepoint for ALL secret-leak detection on the model↔tool boundary. It holds `tokio::sync::Mutex` on both leak detectors (intentional — see comment at runtime_guard.rs:1-21) but re-acquires the same lock four times per `process_outbound` invocation. Under load (multiple concurrent requests), the lock becomes a serialization point. Consider a `parking_lot::Mutex` (sync) held for the duration of the function, OR a per-detector `RwLock` with read-only scan methods.

2. **`extension/plugin_secrets.rs::resolve_settings`** has no rate limit and no cap on recursion depth (the JSON walker is unbounded). The guardrail-side has `MAX_SCAN_DEPTH = 32` (pii_secrets.rs:33), but the plugin-settings path does not — a pathologically nested JSON in `plugins.toml` (an attacker who controls that file) can stack-overflow the recursion in `resolve_value`. The Box::pin recursion is heap-allocated, so it is a memory-DoS not a stack-DoS, but still unbounded.

3. **`extension/runtime/wasm/secret_resolver.rs`** has the comment at line 32-50 documenting that `VaultBackedSecretResolver::for_current_process` is the resolver a plugin load should use. But `plugin_ops.rs:362` constructs a `VaultSecretResolver` directly (NOT through the `for_current_process` factory), so it doesn't use the `VaultBackedSecretResolver`. Two paths to the same vault, one of them correctly threads through `global()` and one doesn't. The `for_current_process` factory should be the only path.

4. **`gateway/interfaces/whatsapp/wa_auth/vault_store.rs`** uses `Arc<Mutex<SecretVault>>` directly. The rest of the codebase holds the vault under `RwLock<SecretVault>` (shared_token.rs:81). Inconsistent locking strategies on the same physical vault file. `WaAuthManager` and `SharedTokenManager` could serialize against each other only via the file-level `VaultIo` lock — but the in-memory state is not synchronized.

5. **`mcp/manager/secret_resolver.rs::resolve_secret_map`** drops the key when resolution fails (line 56-66). This is fail-closed but loses information for the operator. The CLI/audit log records the warning, but the spawned MCP server has no idea a secret was dropped — it may fail with a confusing "missing env var" error far from the cause.

6. **`guardrails/pii_secrets.rs::map_outbound`** strips the secret name from the Block.reason (line 107-122) — good for the model-facing message, but the `tracing::warn` still logs the name verbatim (line 117). The audit log (via `runtime_guard.rs::log_audit`) receives `event_type: ExecBlocked, severity: Critical, detail: "outbound leak blocked..."` — no secret name. An operator triaging needs the name in the audit detail, not in the user-facing message.

7. **`sandbox/scrub.rs::scrub_and_gate_output`** is called from sandbox drivers with `injected: &[]` (line 153) — sandbox output is always fully scrubbed, even when the injected secret was the intentional payload (e.g. a tool that prints its own API key for verification). This is the documented fail-closed behavior, but it means the model cannot learn that its injection succeeded.

8. **`bin/aleph-server/commands/secret.rs::init_locked`** (line 75) silently generates a token without telling the operator the token's value (line 91) — the comment says "The token itself is never returned — only the ready/no-op status is reported." But the operator who runs `aleph-server init` for the first time has no way to retrieve the token to give to the Panel. SECURITY.md §"Token Rotation" implies the operator runs `aleph-server bootstrap-token` separately. Confirm the UX.

9. **`gateway/admin_api/secrets.rs` and `gateway/handlers/secrets.rs`** (the RPC layer) — verify they call `validate_secret_name` before forwarding to `SharedTokenManager::store_secret`. Not directly audited here, but flagged because both files claim to use it. Cross-reference for a future audit.

10. **`hub/secrets.rs::field_key`** uses SHA-256 truncated to 64 bits (line 36) for namespace collision resistance. The docstring says "the collision space is 2^64 and a preimage search against the domain separator is what an attacker would face." 2^64 is below NIST SP 800-131A's 112-bit minimum for collision resistance as of 2024; bump to full 128 bits (16 bytes) or document the threat model more precisely.

11. **The `process_outbound` post-substitution re-scan** (Critical-2) is the single biggest security gap — it is the only point where a freshly substituted secret value could leak to a tool, and it only catches pattern-shaped secrets. Every async call site that goes through `runtime_guard.rs::process_outbound` is affected. This is a CRITICAL fix.

---

## Summary

| Level          | Count |
|----------------|-------|
| Critical       | 6     |
| Warning        | 16    |
| Suggested Test | 6     |

**Cryptographic weaknesses found**: master-key versioning absent (rely on token rotation, not on KDF domain separation); `HKDF_INFO` hard-coded with no migration path; nonce uniqueness relies on `rand::rng()` without explicit collision guard; `EntryMetadata` integrity unprotected; vault `version` accepts downgrades; `VirtualKeyResolver` aliases lack integrity binding; WhatsApp plaintext fallback (Critical-1).

**Leak vectors found**: `resolved_map` String plaintext in runtime_guard; `reset_token` plaintext Vec; per-call `LeakDetector::new()` in http_provider bypasses injected-fingerprint tracking; vault file plaintext via WhatsApp `nonce=0` discriminator; `InjectedSecret::value_len` exposes length side-channel; VaultIo::write may not fsync parent directory on power loss.

**Cross-module concerns**: 11 distinct issues spanning capability, approval, sandbox, guardrails, MCP, WASM plugins, HTTP providers, WhatsApp, and the CLI bootstrap — see the cross-module table above. The single largest gap is Critical-2: `process_outbound`'s post-substitution re-scan only catches pattern-shaped secrets, leaving non-pattern-shaped injected values to reach the model.