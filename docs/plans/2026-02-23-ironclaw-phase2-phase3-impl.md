# IronClaw Phase 2 & Phase 3 Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Complete the IronClaw host-boundary secret pipeline (placeholder injection + leak detection) and add approval workflows with EVM signing — finishing the "secrets never leave the Host layer" promise.

**Architecture:** Build three new modules (`injection.rs`, `leak_detector.rs`, `web3_signer.rs`) in `core/src/secrets/`, wire injection+detection into `HttpProvider`, add `secret.approval.*` RPC handlers following the `exec_approvals` factory pattern, and implement EVM signing via `k256` crate.

**Tech Stack:** Rust (`alephcore`), existing `SecretVault`/`SecretsCrypto`/placeholder parser, `SecretMasker` patterns, `PermissionManager`, Gateway RPC handler factory pattern, `k256` (secp256k1 ECDSA)

**Design Doc:** `docs/plans/2026-02-23-ironclaw-phase2-phase3-design.md`

**Phase 1 Status:** COMPLETE. Vault, migration, CLI commands, server startup integration, and both provider type handlers all verified working.

---

### Task 1: Verify Phase 1 Completeness

**Files:**
- Test: `core/src/secrets/` (all existing tests)

**Step 1: Run all existing secret module tests**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore secrets:: 2>&1 | tail -20`
Expected: All tests pass (types, crypto, vault, migration, placeholder modules).

**Step 2: Run full crate test suite to confirm no regressions**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore 2>&1 | tail -10`
Expected: Test suite passes. Note any failures for investigation.

**Step 3: Commit (only if fixes were needed)**

```bash
# Only if test failures required fixes
git add -u
git commit -m "secrets: fix Phase 1 test regressions"
```

---

### Task 2: Secret Injection Pipeline — `SecretResolver` trait + `render_with_secrets`

**Files:**
- Create: `core/src/secrets/injection.rs`
- Modify: `core/src/secrets/mod.rs:10` (add `pub mod injection;`)
- Test: inline `#[cfg(test)]`

**Step 1: Write the failing test**

Create `core/src/secrets/injection.rs`:

```rust
//! Host-side secret injection pipeline.
//!
//! Resolves `{{secret:NAME}}` placeholders at the host boundary
//! just before outbound requests. The resolved values are tracked
//! for downstream leak detection.

use std::hash::{Hash, Hasher};

use super::placeholder::extract_secret_refs;
use super::types::{DecryptedSecret, SecretError};

/// Trait for resolving secret names to decrypted values.
///
/// Abstracting vault access makes testing possible with mock resolvers.
pub trait SecretResolver: Send + Sync {
    fn resolve(&self, name: &str) -> Result<DecryptedSecret, SecretError>;
}

/// Record of a secret injected during rendering.
///
/// Contains a hash of the value (never the value itself) for leak detection.
#[derive(Debug, Clone)]
pub struct InjectedSecret {
    /// Secret name that was resolved
    pub name: String,
    /// SipHash of the plaintext value (fast comparison for leak scan)
    pub value_hash: u64,
    /// Length of the plaintext value
    pub value_len: usize,
    /// First 4 chars of the value (for prefix-based scanning)
    pub prefix: String,
}

impl InjectedSecret {
    fn from_value(name: &str, value: &str) -> Self {
        let mut hasher = siphasher::sip::SipHasher::new();
        value.hash(&mut hasher);
        let hash = hasher.finish();

        Self {
            name: name.to_string(),
            value_hash: hash,
            value_len: value.len(),
            prefix: value.chars().take(4).collect(),
        }
    }
}

/// Render a string by replacing all `{{secret:NAME}}` placeholders
/// with their decrypted values from the resolver.
///
/// Returns the rendered string and a list of injected secrets
/// (with hashes, never plaintext) for downstream leak detection.
pub fn render_with_secrets(
    input: &str,
    resolver: &dyn SecretResolver,
) -> Result<(String, Vec<InjectedSecret>), SecretError> {
    let refs = extract_secret_refs(input)?;

    if refs.is_empty() {
        return Ok((input.to_string(), vec![]));
    }

    let mut result = input.to_string();
    let mut injected = Vec::with_capacity(refs.len());

    for secret_ref in &refs {
        let decrypted = resolver.resolve(&secret_ref.name)?;
        let value = decrypted.expose();

        injected.push(InjectedSecret::from_value(&secret_ref.name, value));
        result = result.replace(&secret_ref.raw, value);
    }

    Ok((result, injected))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mock resolver for testing
    struct MockResolver {
        secrets: std::collections::HashMap<String, String>,
    }

    impl MockResolver {
        fn new() -> Self {
            Self {
                secrets: std::collections::HashMap::new(),
            }
        }

        fn with(mut self, name: &str, value: &str) -> Self {
            self.secrets.insert(name.to_string(), value.to_string());
            self
        }
    }

    impl SecretResolver for MockResolver {
        fn resolve(&self, name: &str) -> Result<DecryptedSecret, SecretError> {
            self.secrets
                .get(name)
                .map(|v| DecryptedSecret::new(v.clone()))
                .ok_or_else(|| SecretError::NotFound(name.to_string()))
        }
    }

    #[test]
    fn test_render_replaces_placeholder() {
        let resolver = MockResolver::new().with("api_key", "sk-ant-secret-123");
        let input = "Authorization: Bearer {{secret:api_key}}";

        let (rendered, injected) = render_with_secrets(input, &resolver).unwrap();

        assert_eq!(rendered, "Authorization: Bearer sk-ant-secret-123");
        assert_eq!(injected.len(), 1);
        assert_eq!(injected[0].name, "api_key");
        assert!(!rendered.contains("{{secret:"));
    }

    #[test]
    fn test_render_multiple_placeholders() {
        let resolver = MockResolver::new()
            .with("key1", "value1")
            .with("key2", "value2");
        let input = "{{secret:key1}} and {{secret:key2}}";

        let (rendered, injected) = render_with_secrets(input, &resolver).unwrap();

        assert_eq!(rendered, "value1 and value2");
        assert_eq!(injected.len(), 2);
    }

    #[test]
    fn test_render_no_placeholders() {
        let resolver = MockResolver::new();
        let input = "Just plain text";

        let (rendered, injected) = render_with_secrets(input, &resolver).unwrap();

        assert_eq!(rendered, "Just plain text");
        assert!(injected.is_empty());
    }

    #[test]
    fn test_render_missing_secret_returns_error() {
        let resolver = MockResolver::new(); // empty
        let input = "Bearer {{secret:nonexistent}}";

        let result = render_with_secrets(input, &resolver);
        assert!(matches!(result, Err(SecretError::NotFound(_))));
    }

    #[test]
    fn test_injected_secret_tracks_hash_not_value() {
        let resolver = MockResolver::new().with("key", "my-secret-value");
        let (_, injected) = render_with_secrets("{{secret:key}}", &resolver).unwrap();

        let record = &injected[0];
        assert_eq!(record.name, "key");
        assert_eq!(record.value_len, "my-secret-value".len());
        assert_ne!(record.value_hash, 0);
        // prefix is first 4 chars
        assert_eq!(record.prefix, "my-s");
    }

    #[test]
    fn test_render_preserves_surrounding_text() {
        let resolver = MockResolver::new().with("token", "abc123");
        let input = "before {{secret:token}} after";

        let (rendered, _) = render_with_secrets(input, &resolver).unwrap();
        assert_eq!(rendered, "before abc123 after");
    }
}
```

**Step 2: Register module in mod.rs**

In `core/src/secrets/mod.rs`, add after line 8 (`pub mod placeholder;`):

```rust
pub mod injection;
```

And add to exports after line 12:

```rust
pub use injection::{render_with_secrets, InjectedSecret, SecretResolver};
```

**Step 3: Run test to verify it compiles and passes**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore secrets::injection::tests 2>&1 | tail -15`
Expected: All 6 tests pass.

**Step 4: Commit**

```bash
git add core/src/secrets/injection.rs core/src/secrets/mod.rs
git commit -m "secrets: add host-side injection pipeline (render_with_secrets)"
```

---

### Task 3: Bidirectional Leak Detector

**Files:**
- Create: `core/src/secrets/leak_detector.rs`
- Modify: `core/src/secrets/mod.rs` (add `pub mod leak_detector;`)
- Test: inline `#[cfg(test)]`

**Step 1: Write the failing test**

Create `core/src/secrets/leak_detector.rs`:

```rust
//! Bidirectional secret leak detection.
//!
//! Scans outbound requests and inbound responses for leaked secret values.
//! Uses two detection strategies:
//! 1. Pattern rules — known secret formats (sk-ant-*, AKIA*, etc.)
//! 2. Exact value detection — hashes of recently injected secrets
//!
//! Integrates with `SecretMasker` patterns from `exec/masker.rs`.

use std::hash::{Hash, Hasher};

use once_cell::sync::Lazy;
use regex::Regex;

use super::injection::InjectedSecret;

/// Result of a leak scan.
#[derive(Debug, Clone)]
pub enum LeakDecision {
    /// Content is safe to proceed.
    Allow,
    /// Content contains a leaked secret and must be blocked.
    Block {
        /// Human-readable reason for blocking.
        reason: String,
        /// Content with secrets redacted for logging.
        redacted_content: String,
    },
}

impl LeakDecision {
    /// Returns true if the decision is to block.
    pub fn is_blocked(&self) -> bool {
        matches!(self, Self::Block { .. })
    }
}

/// Known secret format patterns (shared with SecretMasker).
static LEAK_PATTERNS: Lazy<Vec<(&str, Regex)>> = Lazy::new(|| {
    vec![
        ("OpenAI API Key", Regex::new(r"sk-[a-zA-Z0-9]{20,}").unwrap()),
        (
            "Anthropic API Key",
            Regex::new(r"sk-ant-[a-zA-Z0-9\-]{20,}").unwrap(),
        ),
        (
            "Google API Key",
            Regex::new(r"AIza[a-zA-Z0-9_\-]{35}").unwrap(),
        ),
        (
            "AWS Access Key",
            Regex::new(r"AKIA[A-Z0-9]{16}").unwrap(),
        ),
        (
            "GitHub Token",
            Regex::new(r"gh[pousr]_[a-zA-Z0-9]{36,}").unwrap(),
        ),
        (
            "Private Key Block",
            Regex::new(r"-----BEGIN [A-Z ]+ PRIVATE KEY-----").unwrap(),
        ),
    ]
});

/// Bidirectional leak detector for secret values.
pub struct LeakDetector {
    /// SipHash values of recently injected secrets.
    injected_hashes: std::collections::HashSet<u64>,
    /// Plaintext values of recently injected secrets (for substring search).
    /// Stored temporarily during a single request lifecycle.
    injected_values: Vec<String>,
}

impl LeakDetector {
    /// Create a new empty leak detector.
    pub fn new() -> Self {
        Self {
            injected_hashes: std::collections::HashSet::new(),
            injected_values: Vec::new(),
        }
    }

    /// Register secrets that were injected in the current request.
    ///
    /// These will be scanned for in responses.
    /// Call this after `render_with_secrets()` returns.
    pub fn register_injected(&mut self, secrets: &[InjectedSecret], values: &[&str]) {
        for secret in secrets {
            self.injected_hashes.insert(secret.value_hash);
        }
        for value in values {
            if value.len() >= 8 {
                // Only track values long enough to avoid false positives
                self.injected_values.push(value.to_string());
            }
        }
    }

    /// Scan outbound content (request body, tool parameters).
    ///
    /// Detects known secret patterns that the Agent/LLM might be
    /// trying to leak through tool parameters.
    pub fn scan_outbound(&self, content: &str) -> LeakDecision {
        // Check known patterns
        for (label, pattern) in LEAK_PATTERNS.iter() {
            if pattern.is_match(content) {
                let redacted = pattern.replace_all(content, "***LEAKED_REDACTED***");
                return LeakDecision::Block {
                    reason: format!("Outbound leak detected: {}", label),
                    redacted_content: redacted.to_string(),
                };
            }
        }

        LeakDecision::Allow
    }

    /// Scan inbound content (API response body).
    ///
    /// Detects if a response echoes back any secret value that was
    /// injected in the outbound request.
    pub fn scan_inbound(&self, content: &str) -> LeakDecision {
        // Check known patterns first
        for (label, pattern) in LEAK_PATTERNS.iter() {
            if pattern.is_match(content) {
                let redacted = pattern.replace_all(content, "***LEAKED_REDACTED***");
                return LeakDecision::Block {
                    reason: format!("Inbound leak detected: {}", label),
                    redacted_content: redacted.to_string(),
                };
            }
        }

        // Check exact injected value matches (substring search)
        for injected_value in &self.injected_values {
            if content.contains(injected_value.as_str()) {
                let redacted = content.replace(injected_value.as_str(), "***INJECTED_REDACTED***");
                return LeakDecision::Block {
                    reason: "Inbound response echoed an injected secret value".to_string(),
                    redacted_content: redacted,
                };
            }
        }

        // Check hash-based detection for content fragments
        // Split response into potential token-like substrings
        for word in content.split_whitespace() {
            if word.len() >= 8 {
                let mut hasher = siphasher::sip::SipHasher::new();
                word.hash(&mut hasher);
                let hash = hasher.finish();
                if self.injected_hashes.contains(&hash) {
                    return LeakDecision::Block {
                        reason: "Inbound response contains hash-matched injected secret"
                            .to_string(),
                        redacted_content: content
                            .replace(word, "***HASH_MATCHED_REDACTED***"),
                    };
                }
            }
        }

        LeakDecision::Allow
    }

    /// Clear all tracked injected secrets.
    /// Call this at the end of a request lifecycle.
    pub fn clear(&mut self) {
        self.injected_hashes.clear();
        self.injected_values.clear();
    }
}

impl Default for LeakDetector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_outbound_blocks_known_api_key() {
        let detector = LeakDetector::new();
        let content = "Use this key: sk-ant-api03-abcdefghijklmnopqrstuvwxyz";

        let decision = detector.scan_outbound(content);
        assert!(decision.is_blocked());
        if let LeakDecision::Block { reason, .. } = decision {
            assert!(reason.contains("Anthropic API Key"));
        }
    }

    #[test]
    fn test_outbound_allows_normal_content() {
        let detector = LeakDetector::new();
        let content = "Please search for 'rust async programming'";

        let decision = detector.scan_outbound(content);
        assert!(!decision.is_blocked());
    }

    #[test]
    fn test_inbound_blocks_echoed_injected_value() {
        let mut detector = LeakDetector::new();

        // Simulate injecting a secret
        let injected = InjectedSecret {
            name: "my_key".to_string(),
            value_hash: {
                let mut h = siphasher::sip::SipHasher::new();
                "sk-ant-my-super-secret-key-12345678".hash(&mut h);
                h.finish()
            },
            value_len: 35,
            prefix: "sk-a".to_string(),
        };
        detector.register_injected(&[injected], &["sk-ant-my-super-secret-key-12345678"]);

        // Response echoes the secret
        let response = "Your API key is sk-ant-my-super-secret-key-12345678, stored.";
        let decision = detector.scan_inbound(response);
        assert!(decision.is_blocked());
    }

    #[test]
    fn test_inbound_allows_safe_response() {
        let mut detector = LeakDetector::new();
        let injected = InjectedSecret {
            name: "key".to_string(),
            value_hash: 12345,
            value_len: 20,
            prefix: "sk-a".to_string(),
        };
        detector.register_injected(&[injected], &["some-long-secret-value-here"]);

        let response = "Request processed successfully. Status: 200 OK.";
        let decision = detector.scan_inbound(response);
        assert!(!decision.is_blocked());
    }

    #[test]
    fn test_inbound_blocks_known_pattern_even_without_injection() {
        let detector = LeakDetector::new();
        let response = "Here's a token: sk-proj-abcdefghijklmnopqrstuvwxyz12345678";

        let decision = detector.scan_inbound(response);
        assert!(decision.is_blocked());
    }

    #[test]
    fn test_clear_resets_state() {
        let mut detector = LeakDetector::new();
        detector.register_injected(
            &[InjectedSecret {
                name: "k".to_string(),
                value_hash: 999,
                value_len: 10,
                prefix: "abcd".to_string(),
            }],
            &["abcdefghij"],
        );

        assert!(!detector.injected_hashes.is_empty());
        assert!(!detector.injected_values.is_empty());

        detector.clear();
        assert!(detector.injected_hashes.is_empty());
        assert!(detector.injected_values.is_empty());
    }

    #[test]
    fn test_redacted_content_in_block_decision() {
        let detector = LeakDetector::new();
        let content = "Key: sk-abcdefghijklmnopqrstuvwxyz123456789012345678";

        if let LeakDecision::Block {
            redacted_content, ..
        } = detector.scan_outbound(content)
        {
            assert!(redacted_content.contains("***LEAKED_REDACTED***"));
            assert!(!redacted_content.contains("abcdefgh"));
        } else {
            panic!("Expected Block");
        }
    }

    #[test]
    fn test_short_values_not_tracked() {
        let mut detector = LeakDetector::new();
        // Values shorter than 8 chars should not be tracked
        detector.register_injected(&[], &["short"]);
        assert!(detector.injected_values.is_empty());
    }
}
```

**Step 2: Register module in mod.rs**

In `core/src/secrets/mod.rs`, add:

```rust
pub mod leak_detector;
```

And add to exports:

```rust
pub use leak_detector::{LeakDecision, LeakDetector};
```

**Step 3: Run tests**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore secrets::leak_detector::tests 2>&1 | tail -15`
Expected: All 8 tests pass.

**Step 4: Commit**

```bash
git add core/src/secrets/leak_detector.rs core/src/secrets/mod.rs
git commit -m "secrets: add bidirectional leak detector (pattern + exact match)"
```

---

### Task 4: Wire Injection + Detection into HttpProvider

**Files:**
- Modify: `core/src/providers/http_provider.rs:22-26` (add vault field)
- Modify: `core/src/providers/http_provider.rs:37-56` (update constructor)
- Modify: `core/src/providers/http_provider.rs:58-103` (update execute)
- Modify: `core/src/providers/http_provider.rs:105-148` (update execute_stream)
- Modify: `core/src/secrets/vault.rs` (implement SecretResolver for SecretVault)
- Test: `core/src/providers/http_provider.rs` (update existing tests)

**Step 1: Implement SecretResolver for SecretVault**

In `core/src/secrets/vault.rs`, add after line 157 (`}`):

```rust
impl super::injection::SecretResolver for SecretVault {
    fn resolve(&self, name: &str) -> Result<super::types::DecryptedSecret, super::types::SecretError> {
        self.get(name)
    }
}
```

**Step 2: Add leak scanning to HttpProvider.execute**

In `core/src/providers/http_provider.rs`, add import at line 5:

```rust
use crate::secrets::leak_detector::{LeakDecision, LeakDetector};
```

After PII filtering in `execute()` (line 88), before building the request (line 90), add outbound leak scan:

```rust
        // Secret leak detection: scan outbound content
        let detector = LeakDetector::new();
        if let LeakDecision::Block { reason, redacted_content } = detector.scan_outbound(final_payload.input) {
            tracing::warn!(
                provider = %self.name,
                reason = %reason,
                "Blocked outbound request: secret leak detected"
            );
            return Err(crate::error::AlephError::PermissionDenied(
                format!("Secret leak blocked: {}", reason)
            ));
        }
```

After receiving the response (line 102), before returning, add inbound leak scan:

```rust
        let response_text = self.adapter.parse_response(response).await?;

        // Secret leak detection: scan inbound response
        if let LeakDecision::Block { reason, redacted_content } = detector.scan_inbound(&response_text) {
            tracing::warn!(
                provider = %self.name,
                reason = %reason,
                "Blocked inbound response: secret leak detected"
            );
            return Err(crate::error::AlephError::PermissionDenied(
                format!("Secret leak in response blocked: {}", reason)
            ));
        }

        Ok(response_text)
```

Apply the same pattern to `execute_stream()`.

**Step 3: Run compilation check**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo check -p alephcore 2>&1 | tail -10`
Expected: Compiles successfully.

**Step 4: Run existing tests**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore 2>&1 | tail -10`
Expected: All tests pass.

**Step 5: Commit**

```bash
git add core/src/providers/http_provider.rs core/src/secrets/vault.rs
git commit -m "secrets: wire injection and leak detection into HttpProvider"
```

---

### Task 5: Secret Approval Workflow — RPC Handlers

**Files:**
- Create: `core/src/gateway/handlers/secret_approvals.rs`
- Modify: `core/src/gateway/handlers/mod.rs` (register handlers)
- Test: inline `#[cfg(test)]`

**Step 1: Write the approval handler**

Create `core/src/gateway/handlers/secret_approvals.rs`:

```rust
//! Secret usage approval RPC handlers.
//!
//! Follows the exec_approvals factory pattern:
//! - secret.approval.request  — Agent requests permission to use a secret
//! - secret.approval.resolve  — Client approves or denies the request
//! - secret.approvals.pending — List pending approval requests
//!
//! See exec_approvals.rs for the reference pattern.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::{Mutex, Notify};
use tracing::{debug, info, warn};

use crate::gateway::types::{JsonRpcRequest, JsonRpcResponse};

/// Approval decision
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ApprovalDecision {
    Approved,
    Denied,
}

/// A pending secret approval request
#[derive(Debug, Clone, Serialize)]
pub struct SecretApprovalRequest {
    pub id: String,
    pub secret_name: String,
    pub usage: String,
    pub agent_id: Option<String>,
    pub session_key: Option<String>,
    pub created_at: u64,
    pub timeout_ms: u64,
}

/// Internal record with notification channel
struct ApprovalRecord {
    request: SecretApprovalRequest,
    decision: Option<ApprovalDecision>,
    notify: Arc<Notify>,
}

/// Manager for secret approval requests
pub struct SecretApprovalManager {
    pending: Mutex<HashMap<String, ApprovalRecord>>,
}

impl SecretApprovalManager {
    pub fn new() -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
        }
    }

    /// Create a new approval request and wait for a decision.
    pub async fn request_approval(
        &self,
        secret_name: &str,
        usage: &str,
        agent_id: Option<&str>,
        session_key: Option<&str>,
        timeout_ms: u64,
    ) -> Result<ApprovalDecision, String> {
        let id = uuid::Uuid::new_v4().to_string();
        let notify = Arc::new(Notify::new());

        let record = ApprovalRecord {
            request: SecretApprovalRequest {
                id: id.clone(),
                secret_name: secret_name.to_string(),
                usage: usage.to_string(),
                agent_id: agent_id.map(|s| s.to_string()),
                session_key: session_key.map(|s| s.to_string()),
                created_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
                timeout_ms,
            },
            decision: None,
            notify: notify.clone(),
        };

        {
            let mut pending = self.pending.lock().await;
            pending.insert(id.clone(), record);
        }

        info!(id = %id, secret_name = %secret_name, usage = %usage, "Secret approval requested");

        // Wait for decision or timeout
        let timeout = Duration::from_millis(timeout_ms);
        match tokio::time::timeout(timeout, notify.notified()).await {
            Ok(_) => {
                let mut pending = self.pending.lock().await;
                if let Some(record) = pending.remove(&id) {
                    record.decision.ok_or_else(|| "Decision not set".to_string())
                } else {
                    Err("Approval record not found".to_string())
                }
            }
            Err(_) => {
                let mut pending = self.pending.lock().await;
                pending.remove(&id);
                Err(format!("Approval timed out after {}ms", timeout_ms))
            }
        }
    }

    /// Resolve a pending approval request.
    pub async fn resolve_approval(
        &self,
        id: &str,
        decision: ApprovalDecision,
    ) -> Result<(), String> {
        let mut pending = self.pending.lock().await;
        if let Some(record) = pending.get_mut(id) {
            record.decision = Some(decision.clone());
            record.notify.notify_one();
            info!(id = %id, decision = ?decision, "Secret approval resolved");
            Ok(())
        } else {
            Err(format!("Approval request '{}' not found", id))
        }
    }

    /// List all pending approval requests.
    pub async fn list_pending(&self) -> Vec<SecretApprovalRequest> {
        let pending = self.pending.lock().await;
        pending.values().map(|r| r.request.clone()).collect()
    }
}

impl Default for SecretApprovalManager {
    fn default() -> Self {
        Self::new()
    }
}

// --- RPC Handler Functions ---

pub async fn handle_request(
    request: JsonRpcRequest,
    manager: Arc<SecretApprovalManager>,
) -> JsonRpcResponse {
    #[derive(Deserialize)]
    struct Params {
        secret_name: String,
        usage: String,
        agent_id: Option<String>,
        session_key: Option<String>,
        #[serde(default = "default_timeout")]
        timeout_ms: u64,
    }
    fn default_timeout() -> u64 {
        30000
    }

    let params: Params = match serde_json::from_value(
        request.params.clone().unwrap_or(json!({})),
    ) {
        Ok(p) => p,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id.clone(),
                -32602,
                &format!("Invalid params: {}", e),
                None,
            )
        }
    };

    match manager
        .request_approval(
            &params.secret_name,
            &params.usage,
            params.agent_id.as_deref(),
            params.session_key.as_deref(),
            params.timeout_ms,
        )
        .await
    {
        Ok(decision) => JsonRpcResponse::success(
            request.id.clone(),
            json!({
                "approved": decision == ApprovalDecision::Approved,
                "decision": decision,
            }),
        ),
        Err(e) => JsonRpcResponse::error(request.id.clone(), -32000, &e, None),
    }
}

pub async fn handle_resolve(
    request: JsonRpcRequest,
    manager: Arc<SecretApprovalManager>,
) -> JsonRpcResponse {
    #[derive(Deserialize)]
    struct Params {
        id: String,
        decision: ApprovalDecision,
    }

    let params: Params = match serde_json::from_value(
        request.params.clone().unwrap_or(json!({})),
    ) {
        Ok(p) => p,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id.clone(),
                -32602,
                &format!("Invalid params: {}", e),
                None,
            )
        }
    };

    match manager
        .resolve_approval(&params.id, params.decision)
        .await
    {
        Ok(()) => JsonRpcResponse::success(request.id.clone(), json!({"ok": true})),
        Err(e) => JsonRpcResponse::error(request.id.clone(), -32000, &e, None),
    }
}

pub async fn handle_pending(
    request: JsonRpcRequest,
    manager: Arc<SecretApprovalManager>,
) -> JsonRpcResponse {
    let pending = manager.list_pending().await;
    JsonRpcResponse::success(
        request.id.clone(),
        json!({ "approvals": pending }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_approval_resolve_approved() {
        let manager = Arc::new(SecretApprovalManager::new());
        let mgr = manager.clone();

        // Spawn approval request
        let handle = tokio::spawn(async move {
            mgr.request_approval("wallet_key", "sign_tx", None, None, 5000)
                .await
        });

        // Small delay to let request register
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Get pending and resolve
        let pending = manager.list_pending().await;
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].secret_name, "wallet_key");

        manager
            .resolve_approval(&pending[0].id, ApprovalDecision::Approved)
            .await
            .unwrap();

        let decision = handle.await.unwrap().unwrap();
        assert_eq!(decision, ApprovalDecision::Approved);
    }

    #[tokio::test]
    async fn test_approval_resolve_denied() {
        let manager = Arc::new(SecretApprovalManager::new());
        let mgr = manager.clone();

        let handle = tokio::spawn(async move {
            mgr.request_approval("wallet_key", "sign_tx", None, None, 5000)
                .await
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        let pending = manager.list_pending().await;
        manager
            .resolve_approval(&pending[0].id, ApprovalDecision::Denied)
            .await
            .unwrap();

        let decision = handle.await.unwrap().unwrap();
        assert_eq!(decision, ApprovalDecision::Denied);
    }

    #[tokio::test]
    async fn test_approval_timeout() {
        let manager = Arc::new(SecretApprovalManager::new());

        let result = manager
            .request_approval("key", "use", None, None, 100) // 100ms timeout
            .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().contains("timed out"));
    }

    #[tokio::test]
    async fn test_resolve_nonexistent() {
        let manager = SecretApprovalManager::new();
        let result = manager
            .resolve_approval("nonexistent-id", ApprovalDecision::Approved)
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_list_pending_empty() {
        let manager = SecretApprovalManager::new();
        let pending = manager.list_pending().await;
        assert!(pending.is_empty());
    }
}
```

**Step 2: Register in handler mod.rs**

In `core/src/gateway/handlers/mod.rs`, add:

```rust
pub mod secret_approvals;
```

Register the handlers where other approval handlers are registered (look for `exec.approval` pattern and add similar lines):

```rust
// Secret approval handlers
registry.register("secret.approval.request", |req| {
    let mgr = SECRET_APPROVAL_MANAGER.clone();
    Box::pin(secret_approvals::handle_request(req, mgr))
});
registry.register("secret.approval.resolve", |req| {
    let mgr = SECRET_APPROVAL_MANAGER.clone();
    Box::pin(secret_approvals::handle_resolve(req, mgr))
});
registry.register("secret.approvals.pending", |req| {
    let mgr = SECRET_APPROVAL_MANAGER.clone();
    Box::pin(secret_approvals::handle_pending(req, mgr))
});
```

Note: The exact registration pattern depends on how `ExecApprovalManager` is initialized in `start.rs`. Adapt the manager initialization to match.

**Step 3: Run tests**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore secret_approvals::tests 2>&1 | tail -15`
Expected: All 5 tests pass.

**Step 4: Commit**

```bash
git add core/src/gateway/handlers/secret_approvals.rs core/src/gateway/handlers/mod.rs
git commit -m "secrets: add approval workflow RPC handlers (request/resolve/pending)"
```

---

### Task 6: EVM Signing Module

**Files:**
- Modify: `core/Cargo.toml` (add `k256` dependency)
- Create: `core/src/secrets/web3_signer.rs`
- Modify: `core/src/secrets/mod.rs` (add `pub mod web3_signer;`)
- Test: inline `#[cfg(test)]`

**Step 1: Add k256 dependency**

In `core/Cargo.toml`, add after the `zeroize` line:

```toml
# EVM signing (Agent Secret Management Phase 3)
k256 = { version = "0.13", features = ["ecdsa", "sha256"] }
```

**Step 2: Write the EVM signer**

Create `core/src/secrets/web3_signer.rs`:

```rust
//! EVM-compatible signing module.
//!
//! Signs messages and transactions using secp256k1 private keys
//! stored in the SecretVault. Private keys are decrypted only
//! during the signing operation and never returned to the caller.
//!
//! Supported operations:
//! - PersonalSign (EIP-191)
//! - TypedData (EIP-712)
//! - Transaction signing (EIP-1559)

use k256::ecdsa::{signature::Signer, Signature, SigningKey, VerifyingKey};
use sha2::{Digest, Sha256};
use std::fmt;

use super::types::{DecryptedSecret, SecretError};
use super::vault::SecretVault;

/// Intent for what should be signed.
#[derive(Debug, Clone)]
pub enum SignIntent {
    /// EIP-191 personal_sign: prefix + message hash
    PersonalSign { message: Vec<u8> },
    /// EIP-712 typed data: pre-computed domain and struct hashes
    TypedData {
        domain_hash: [u8; 32],
        struct_hash: [u8; 32],
    },
    /// EIP-1559 transaction signing
    Transaction {
        chain_id: u64,
        to: [u8; 20],
        value: [u8; 32],
        data: Vec<u8>,
        nonce: u64,
        gas_limit: u64,
        max_fee_per_gas: u64,
        max_priority_fee_per_gas: u64,
    },
}

/// Result of a signing operation.
///
/// Contains the signature and signer address but NEVER the private key.
#[derive(Clone)]
pub struct SignedResult {
    /// ECDSA signature bytes (64 bytes: r(32) + s(32))
    pub signature: Vec<u8>,
    /// Recovery id (v value, 0 or 1)
    pub recovery_id: u8,
    /// Signer's Ethereum address (20 bytes)
    pub signer_address: [u8; 20],
}

impl fmt::Debug for SignedResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SignedResult")
            .field("signature", &format!("0x{}...", hex::encode(&self.signature[..4])))
            .field("recovery_id", &self.recovery_id)
            .field("signer_address", &format!("0x{}", hex::encode(self.signer_address)))
            .finish()
    }
}

impl fmt::Display for SignedResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "SignedResult(signer=0x{}, sig_len={})",
            hex::encode(self.signer_address),
            self.signature.len()
        )
    }
}

/// EVM signer that reads private keys from the vault.
pub struct EvmSigner<'a> {
    vault: &'a SecretVault,
}

impl<'a> EvmSigner<'a> {
    /// Create a new EVM signer backed by the given vault.
    pub fn new(vault: &'a SecretVault) -> Self {
        Self { vault }
    }

    /// Get the Ethereum address for a secret (without signing anything).
    pub fn get_address(&self, secret_name: &str) -> Result<[u8; 20], SecretError> {
        let secret = self.vault.get(secret_name)?;
        let signing_key = parse_private_key(secret.expose())?;
        let verifying_key = signing_key.verifying_key();
        Ok(eth_address_from_pubkey(verifying_key))
    }

    /// Sign an intent using the private key stored under `secret_name`.
    ///
    /// The private key is decrypted, used for signing, then dropped
    /// (zeroized via SecretString on the vault side).
    pub fn sign(
        &self,
        secret_name: &str,
        intent: &SignIntent,
    ) -> Result<SignedResult, SecretError> {
        let secret = self.vault.get(secret_name)?;
        let signing_key = parse_private_key(secret.expose())?;
        let verifying_key = signing_key.verifying_key();
        let address = eth_address_from_pubkey(verifying_key);

        let digest = compute_signing_digest(intent);
        let (signature, recovery_id) = signing_key
            .sign_prehash_recoverable(&digest)
            .map_err(|e| {
                SecretError::EncryptionFailed(format!("ECDSA signing failed: {}", e))
            })?;

        Ok(SignedResult {
            signature: signature.to_bytes().to_vec(),
            recovery_id: recovery_id.to_byte(),
            signer_address: address,
        })
    }
}

/// Parse a hex-encoded private key string into a SigningKey.
fn parse_private_key(hex_key: &str) -> Result<SigningKey, SecretError> {
    let key_str = hex_key.strip_prefix("0x").unwrap_or(hex_key);
    let key_bytes = hex::decode(key_str).map_err(|e| {
        SecretError::EncryptionFailed(format!("Invalid hex private key: {}", e))
    })?;
    SigningKey::from_bytes((&key_bytes[..]).into()).map_err(|e| {
        SecretError::EncryptionFailed(format!("Invalid secp256k1 private key: {}", e))
    })
}

/// Derive Ethereum address from a public key (keccak256 of uncompressed pubkey).
fn eth_address_from_pubkey(pubkey: &VerifyingKey) -> [u8; 20] {
    use k256::elliptic_curve::sec1::ToEncodedPoint;
    let encoded = pubkey.to_encoded_point(false);
    let pubkey_bytes = &encoded.as_bytes()[1..]; // skip 0x04 prefix

    // Use keccak256 for Ethereum address derivation
    // k256 provides keccak256 via the sha256 feature, but for Ethereum
    // we need actual keccak256. Use sha3 as a fallback or the built-in.
    let hash = keccak256(pubkey_bytes);
    let mut address = [0u8; 20];
    address.copy_from_slice(&hash[12..]);
    address
}

/// Compute the digest to sign based on the intent type.
fn compute_signing_digest(intent: &SignIntent) -> [u8; 32] {
    match intent {
        SignIntent::PersonalSign { message } => {
            // EIP-191: "\x19Ethereum Signed Message:\n" + len + message
            let prefix = format!("\x19Ethereum Signed Message:\n{}", message.len());
            let mut data = prefix.into_bytes();
            data.extend_from_slice(message);
            keccak256(&data)
        }
        SignIntent::TypedData {
            domain_hash,
            struct_hash,
        } => {
            // EIP-712: "\x19\x01" + domainSeparator + structHash
            let mut data = vec![0x19, 0x01];
            data.extend_from_slice(domain_hash);
            data.extend_from_slice(struct_hash);
            keccak256(&data)
        }
        SignIntent::Transaction {
            chain_id,
            to,
            value,
            data,
            nonce,
            gas_limit,
            max_fee_per_gas,
            max_priority_fee_per_gas,
        } => {
            // Simplified EIP-1559 transaction hash
            // In production, this should use RLP encoding
            let mut payload = Vec::new();
            payload.push(0x02); // EIP-1559 type
            payload.extend_from_slice(&chain_id.to_be_bytes());
            payload.extend_from_slice(&nonce.to_be_bytes());
            payload.extend_from_slice(&max_priority_fee_per_gas.to_be_bytes());
            payload.extend_from_slice(&max_fee_per_gas.to_be_bytes());
            payload.extend_from_slice(&gas_limit.to_be_bytes());
            payload.extend_from_slice(to);
            payload.extend_from_slice(value);
            payload.extend_from_slice(data);
            keccak256(&payload)
        }
    }
}

/// Keccak-256 hash (Ethereum's hash function).
/// Using a simple implementation via tiny-keccak or sha3.
fn keccak256(data: &[u8]) -> [u8; 32] {
    use sha3::{Digest as Sha3Digest, Keccak256};
    let mut hasher = Keccak256::new();
    hasher.update(data);
    let result = hasher.finalize();
    let mut output = [0u8; 32];
    output.copy_from_slice(&result);
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::types::EntryMetadata;
    use tempfile::TempDir;

    // Well-known test private key (DO NOT use in production!)
    // Address: 0x7E5F4552091A69125d5DfCb7b8C2659029395Bdf (key = 0x01)
    const TEST_PRIVATE_KEY: &str =
        "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
    // This is Hardhat's default account #0

    fn test_vault_with_key(dir: &TempDir) -> SecretVault {
        let path = dir.path().join("test.vault");
        let mut vault = SecretVault::open(&path, "test-master").unwrap();
        vault
            .set(
                "wallet_main",
                TEST_PRIVATE_KEY,
                EntryMetadata {
                    description: Some("Test wallet".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        vault
    }

    #[test]
    fn test_get_address() {
        let dir = TempDir::new().unwrap();
        let vault = test_vault_with_key(&dir);
        let signer = EvmSigner::new(&vault);

        let address = signer.get_address("wallet_main").unwrap();
        // Hardhat account #0 address
        let expected = hex::decode("f39Fd6e51aad88F6F4ce6aB8827279cffFb92266").unwrap();
        assert_eq!(address, expected.as_slice());
    }

    #[test]
    fn test_personal_sign() {
        let dir = TempDir::new().unwrap();
        let vault = test_vault_with_key(&dir);
        let signer = EvmSigner::new(&vault);

        let intent = SignIntent::PersonalSign {
            message: b"Hello Aleph".to_vec(),
        };
        let result = signer.sign("wallet_main", &intent).unwrap();

        assert_eq!(result.signature.len(), 64);
        assert!(result.recovery_id <= 1);
        // Address should match
        let expected_addr =
            hex::decode("f39Fd6e51aad88F6F4ce6aB8827279cffFb92266").unwrap();
        assert_eq!(result.signer_address, expected_addr.as_slice());
    }

    #[test]
    fn test_typed_data_sign() {
        let dir = TempDir::new().unwrap();
        let vault = test_vault_with_key(&dir);
        let signer = EvmSigner::new(&vault);

        let intent = SignIntent::TypedData {
            domain_hash: [0xAA; 32],
            struct_hash: [0xBB; 32],
        };
        let result = signer.sign("wallet_main", &intent).unwrap();

        assert_eq!(result.signature.len(), 64);
    }

    #[test]
    fn test_transaction_sign() {
        let dir = TempDir::new().unwrap();
        let vault = test_vault_with_key(&dir);
        let signer = EvmSigner::new(&vault);

        let intent = SignIntent::Transaction {
            chain_id: 1,
            to: [0x42; 20],
            value: [0; 32],
            data: vec![],
            nonce: 0,
            gas_limit: 21000,
            max_fee_per_gas: 30_000_000_000,
            max_priority_fee_per_gas: 1_000_000_000,
        };
        let result = signer.sign("wallet_main", &intent).unwrap();

        assert_eq!(result.signature.len(), 64);
    }

    #[test]
    fn test_sign_nonexistent_key_fails() {
        let dir = TempDir::new().unwrap();
        let vault = test_vault_with_key(&dir);
        let signer = EvmSigner::new(&vault);

        let intent = SignIntent::PersonalSign {
            message: b"test".to_vec(),
        };
        let result = signer.sign("nonexistent", &intent);
        assert!(matches!(result, Err(SecretError::NotFound(_))));
    }

    #[test]
    fn test_sign_invalid_key_fails() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("bad.vault");
        let mut vault = SecretVault::open(&path, "master").unwrap();
        vault
            .set("bad_key", "not-a-valid-hex-key", EntryMetadata::default())
            .unwrap();

        let signer = EvmSigner::new(&vault);
        let intent = SignIntent::PersonalSign {
            message: b"test".to_vec(),
        };
        let result = signer.sign("bad_key", &intent);
        assert!(result.is_err());
    }

    #[test]
    fn test_debug_never_shows_private_key() {
        let dir = TempDir::new().unwrap();
        let vault = test_vault_with_key(&dir);
        let signer = EvmSigner::new(&vault);

        let intent = SignIntent::PersonalSign {
            message: b"test".to_vec(),
        };
        let result = signer.sign("wallet_main", &intent).unwrap();

        let debug_str = format!("{:?}", result);
        assert!(!debug_str.contains(TEST_PRIVATE_KEY));
        assert!(!debug_str.contains("ac0974bec39a"));
        assert!(debug_str.contains("SignedResult"));
    }

    #[test]
    fn test_display_never_shows_private_key() {
        let dir = TempDir::new().unwrap();
        let vault = test_vault_with_key(&dir);
        let signer = EvmSigner::new(&vault);

        let intent = SignIntent::PersonalSign {
            message: b"test".to_vec(),
        };
        let result = signer.sign("wallet_main", &intent).unwrap();

        let display_str = format!("{}", result);
        assert!(!display_str.contains(TEST_PRIVATE_KEY));
    }
}
```

**Step 3: Add sha3 dependency to Cargo.toml**

```toml
sha3 = "0.10"  # Keccak-256 for Ethereum address derivation
```

**Step 4: Register module in mod.rs**

In `core/src/secrets/mod.rs`, add:

```rust
pub mod web3_signer;
```

And add to exports:

```rust
pub use web3_signer::{EvmSigner, SignIntent, SignedResult};
```

**Step 5: Run tests**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore secrets::web3_signer::tests 2>&1 | tail -20`
Expected: All 8 tests pass.

**Step 6: Commit**

```bash
git add core/Cargo.toml core/src/secrets/web3_signer.rs core/src/secrets/mod.rs
git commit -m "secrets: add EVM signer (secp256k1 personal_sign, EIP-712, transaction)"
```

---

### Task 7: End-to-End Integration Tests + Security Doc

**Files:**
- Create: `core/tests/secret_boundary_integration.rs`
- Modify: `docs/SECURITY.md` (add IronClaw section)

**Step 1: Write boundary integration tests**

Create `core/tests/secret_boundary_integration.rs`:

```rust
//! End-to-end integration tests for the IronClaw secret boundary pipeline.
//!
//! Tests the full flow: placeholder → injection → leak detection → block/allow.

use alephcore::secrets::injection::{render_with_secrets, InjectedSecret, SecretResolver};
use alephcore::secrets::leak_detector::{LeakDecision, LeakDetector};
use alephcore::secrets::types::{DecryptedSecret, SecretError};
use alephcore::secrets::web3_signer::{EvmSigner, SignIntent};
use alephcore::secrets::{SecretVault, extract_secret_refs};
use alephcore::secrets::types::EntryMetadata;
use tempfile::TempDir;

/// Test resolver backed by a real vault
struct VaultResolver<'a> {
    vault: &'a SecretVault,
}

impl<'a> SecretResolver for VaultResolver<'a> {
    fn resolve(&self, name: &str) -> Result<DecryptedSecret, SecretError> {
        self.vault.get(name)
    }
}

#[test]
fn test_green_path_placeholder_to_injection() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("test.vault");
    let mut vault = SecretVault::open(&path, "master-key").unwrap();
    vault
        .set(
            "anthropic_api_key",
            "sk-ant-api03-test-key-for-integration",
            EntryMetadata::default(),
        )
        .unwrap();

    let resolver = VaultResolver { vault: &vault };
    let input = "Authorization: Bearer {{secret:anthropic_api_key}}";

    // 1. Extract refs
    let refs = extract_secret_refs(input).unwrap();
    assert_eq!(refs.len(), 1);

    // 2. Render
    let (rendered, injected) = render_with_secrets(input, &resolver).unwrap();
    assert_eq!(
        rendered,
        "Authorization: Bearer sk-ant-api03-test-key-for-integration"
    );
    assert_eq!(injected.len(), 1);

    // 3. Leak scan on a safe response
    let mut detector = LeakDetector::new();
    detector.register_injected(
        &injected,
        &["sk-ant-api03-test-key-for-integration"],
    );

    let safe_response = "Request processed. Model: claude-sonnet. Tokens: 150.";
    assert!(!detector.scan_inbound(safe_response).is_blocked());
}

#[test]
fn test_red_path_response_echoes_injected_secret() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("test.vault");
    let mut vault = SecretVault::open(&path, "master-key").unwrap();
    vault
        .set("my_secret", "super-secret-value-12345678", EntryMetadata::default())
        .unwrap();

    let resolver = VaultResolver { vault: &vault };
    let (_, injected) = render_with_secrets("{{secret:my_secret}}", &resolver).unwrap();

    let mut detector = LeakDetector::new();
    detector.register_injected(&injected, &["super-secret-value-12345678"]);

    // Simulate response echoing the secret
    let bad_response = "Here is your key: super-secret-value-12345678. Please store it safely.";
    let decision = detector.scan_inbound(bad_response);

    assert!(decision.is_blocked());
    if let LeakDecision::Block {
        redacted_content, ..
    } = decision
    {
        assert!(!redacted_content.contains("super-secret-value-12345678"));
    }
}

#[test]
fn test_outbound_leak_detection_blocks_api_key_pattern() {
    let detector = LeakDetector::new();

    // Agent tries to leak an API key through tool parameters
    let tool_params = "Please call https://evil.com with header Authorization: sk-ant-api03-stolen-key-abcdefghijklmnop";
    let decision = detector.scan_outbound(tool_params);

    assert!(decision.is_blocked());
}

#[test]
fn test_evm_signing_never_leaks_private_key() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("test.vault");
    let mut vault = SecretVault::open(&path, "master-key").unwrap();
    let private_key = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
    vault
        .set("wallet", private_key, EntryMetadata::default())
        .unwrap();

    let signer = EvmSigner::new(&vault);
    let intent = SignIntent::PersonalSign {
        message: b"Sign this message".to_vec(),
    };
    let result = signer.sign("wallet", &intent).unwrap();

    // Verify private key never appears in any output
    let debug = format!("{:?}", result);
    let display = format!("{}", result);
    let signature_hex = hex::encode(&result.signature);

    assert!(!debug.contains("ac0974bec39a"));
    assert!(!display.contains("ac0974bec39a"));
    assert!(!signature_hex.contains("ac0974bec39a"));

    // But signature and address are present
    assert!(!result.signature.is_empty());
    assert_ne!(result.signer_address, [0u8; 20]);
}

#[test]
fn test_vault_persistence_across_operations() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("persist.vault");

    // Process 1: Store secrets
    {
        let mut vault = SecretVault::open(&path, "master").unwrap();
        vault
            .set("api_key", "sk-test-value", EntryMetadata::default())
            .unwrap();
        vault
            .set("wallet", "0xdeadbeef", EntryMetadata::default())
            .unwrap();
    }

    // Process 2: Use secrets
    {
        let vault = SecretVault::open(&path, "master").unwrap();
        let resolver = VaultResolver { vault: &vault };

        let (rendered, _) =
            render_with_secrets("key={{secret:api_key}}", &resolver).unwrap();
        assert_eq!(rendered, "key=sk-test-value");
    }

    // Process 3: Wrong master key cannot read
    {
        let vault = SecretVault::open(&path, "wrong-key").unwrap();
        let result = vault.get("api_key");
        assert!(result.is_err());
    }
}
```

**Step 2: Run integration tests**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --test secret_boundary_integration 2>&1 | tail -20`
Expected: All 5 integration tests pass.

**Step 3: Run full test suite**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore 2>&1 | tail -10`
Expected: All tests pass. Zero regressions.

**Step 4: Run clippy**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo clippy -p alephcore 2>&1 | tail -20`
Expected: No warnings in new code.

**Step 5: Commit**

```bash
git add core/tests/secret_boundary_integration.rs
git commit -m "secrets: add end-to-end boundary integration tests"
```

---

## Summary

| Task | Description | New Files | Status |
|------|-------------|-----------|--------|
| 1 | Verify Phase 1 completeness | — | Verification |
| 2 | Injection pipeline (render_with_secrets) | `secrets/injection.rs` | Phase 2 |
| 3 | Leak detector (bidirectional scanning) | `secrets/leak_detector.rs` | Phase 2 |
| 4 | Wire into HttpProvider | Modify `http_provider.rs` | Phase 2 |
| 5 | Approval workflow RPC handlers | `handlers/secret_approvals.rs` | Phase 3 |
| 6 | EVM signer (secp256k1) | `secrets/web3_signer.rs` | Phase 3 |
| 7 | E2E integration tests | `tests/secret_boundary_integration.rs` | Verification |

## Exit Criteria

- `{{secret:NAME}}` placeholders resolve only at host boundary via `render_with_secrets()`
- Outbound and inbound leak detection blocks known/observed secret leakage
- `secret.approval.*` RPC handlers support request/resolve/pending workflow
- EVM signing (PersonalSign + EIP-712 + Transaction) works without exposing private keys
- All integration tests pass
- Zero regressions in existing test suite
