# WASM Capability Kernel Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add a default-deny capability model to Aleph's WASM plugin runtime, with 6 host functions, credential injection, leak detection, and trust-based approval integration.

**Architecture:** Build `WasmCapabilityKernel` on top of Extism (kept as runtime), register host functions via `PluginBuilder::with_function()`, enforce capabilities declared in `aleph.plugin.toml`. Each host function call passes through the kernel for permission checks, leak scanning, credential injection, and audit logging.

**Tech Stack:** Extism 1.7 (Wasmtime), aho-corasick (leak detection), reqwest (HTTP proxy), regex (patterns), serde/toml (config parsing)

---

## Task 1: Add `aho-corasick` Dependency

**Files:**
- Modify: `core/Cargo.toml`

**Step 1: Add dependency**

Add `aho-corasick = "1.1"` to `[dependencies]` in `core/Cargo.toml`, right after the existing `regex = "1.10"` line.

```toml
aho-corasick = "1.1"
```

**Step 2: Verify it compiles**

Run: `cd core && cargo check --features plugin-wasm`
Expected: Success (no errors)

**Step 3: Commit**

```bash
git add core/Cargo.toml
git commit -m "deps: add aho-corasick for WASM leak detection"
```

---

## Task 2: Create `WasmCapabilities` Types

**Files:**
- Create: `core/src/extension/runtime/wasm/capabilities.rs`
- Modify: `core/src/extension/runtime/wasm/mod.rs:6` (add `mod capabilities;`)
- Test: `core/src/extension/runtime/wasm/capabilities.rs` (inline tests)

**Step 1: Write the failing test**

Create `core/src/extension/runtime/wasm/capabilities.rs` with only the test module first:

```rust
//! WASM plugin capability types.
//!
//! Defines the capability model for WASM plugins:
//! - Default-deny: no capabilities = zero permissions
//! - Capabilities declared in aleph.plugin.toml [plugin.capabilities]

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_capabilities_are_empty() {
        let caps = WasmCapabilities::default();
        assert!(caps.workspace.is_none());
        assert!(caps.http.is_none());
        assert!(caps.tool_invoke.is_none());
        assert!(caps.secrets.is_none());
    }

    #[test]
    fn test_endpoint_pattern_matches_exact_host() {
        let pattern = EndpointPattern {
            host: "api.slack.com".to_string(),
            path_prefix: "/api/".to_string(),
            methods: vec!["GET".to_string(), "POST".to_string()],
        };
        assert!(pattern.matches("GET", "api.slack.com", "/api/users"));
        assert!(!pattern.matches("GET", "evil.com", "/api/users"));
        assert!(!pattern.matches("DELETE", "api.slack.com", "/api/users"));
        assert!(!pattern.matches("GET", "api.slack.com", "/other/path"));
    }

    #[test]
    fn test_endpoint_pattern_wildcard_host() {
        let pattern = EndpointPattern {
            host: "*.slack.com".to_string(),
            path_prefix: "/".to_string(),
            methods: vec!["GET".to_string()],
        };
        assert!(pattern.matches("GET", "api.slack.com", "/anything"));
        assert!(pattern.matches("GET", "hooks.slack.com", "/webhook"));
        assert!(!pattern.matches("GET", "slack.com", "/root"));
        assert!(!pattern.matches("GET", "evil.slack.com.attacker.com", "/x"));
    }

    #[test]
    fn test_credential_inject_variants() {
        let bearer = CredentialInject::Bearer;
        let basic = CredentialInject::Basic { username: "bot".to_string() };
        let header = CredentialInject::Header {
            name: "X-API-Key".to_string(),
            prefix: Some("Key ".to_string()),
        };
        let query = CredentialInject::Query { param_name: "api_key".to_string() };
        let url_path = CredentialInject::UrlPath {
            placeholder: "{TOKEN}".to_string(),
        };

        // Just verify they can be created (type system test)
        assert!(matches!(bearer, CredentialInject::Bearer));
        assert!(matches!(basic, CredentialInject::Basic { .. }));
        assert!(matches!(header, CredentialInject::Header { .. }));
        assert!(matches!(query, CredentialInject::Query { .. }));
        assert!(matches!(url_path, CredentialInject::UrlPath { .. }));
    }

    #[test]
    fn test_secrets_capability_pattern_matching() {
        let cap = SecretsCapability {
            allowed_patterns: vec!["slack_*".to_string(), "github_token".to_string()],
        };
        assert!(cap.is_allowed("slack_bot_token"));
        assert!(cap.is_allowed("slack_webhook"));
        assert!(cap.is_allowed("github_token"));
        assert!(!cap.is_allowed("aws_secret"));
        assert!(!cap.is_allowed("slackbot")); // no underscore after slack
    }
}
```

**Step 2: Add module declaration**

In `core/src/extension/runtime/wasm/mod.rs`, add after line 6 (`mod permissions;`):
```rust
mod capabilities;
pub use capabilities::WasmCapabilities;
```

**Step 3: Run test to verify it fails**

Run: `cd core && cargo test --features plugin-wasm wasm::capabilities`
Expected: FAIL — types not defined

**Step 4: Write implementation**

Fill in the structs and methods in `capabilities.rs` above the test module:

```rust
use std::collections::HashMap;
use serde::{Deserialize, Serialize};

/// WASM plugin capability configuration (parsed from aleph.plugin.toml)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WasmCapabilities {
    pub workspace: Option<WorkspaceCapability>,
    pub http: Option<HttpCapability>,
    pub tool_invoke: Option<ToolInvokeCapability>,
    pub secrets: Option<SecretsCapability>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceCapability {
    #[serde(default)]
    pub allowed_prefixes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpCapability {
    #[serde(default)]
    pub allowlist: Vec<EndpointPattern>,
    #[serde(default)]
    pub credentials: Vec<CredentialBinding>,
    #[serde(default)]
    pub rate_limit: Option<RateLimit>,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    #[serde(default = "default_max_request_bytes")]
    pub max_request_bytes: usize,
    #[serde(default = "default_max_response_bytes")]
    pub max_response_bytes: usize,
}

fn default_timeout() -> u64 { 30 }
fn default_max_request_bytes() -> usize { 1_048_576 }     // 1MB
fn default_max_response_bytes() -> usize { 10_485_760 }   // 10MB

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EndpointPattern {
    pub host: String,
    #[serde(default = "default_path_prefix")]
    pub path_prefix: String,
    #[serde(default)]
    pub methods: Vec<String>,
}

fn default_path_prefix() -> String { "/".to_string() }

impl EndpointPattern {
    /// Check if a request matches this pattern
    pub fn matches(&self, method: &str, host: &str, path: &str) -> bool {
        // Method check (empty = allow all)
        if !self.methods.is_empty()
            && !self.methods.iter().any(|m| m.eq_ignore_ascii_case(method))
        {
            return false;
        }

        // Host check (supports *.domain.com wildcard)
        if !self.host_matches(host) {
            return false;
        }

        // Path prefix check
        path.starts_with(&self.path_prefix)
    }

    fn host_matches(&self, host: &str) -> bool {
        if let Some(suffix) = self.host.strip_prefix("*.") {
            // Wildcard: *.slack.com matches api.slack.com but not slack.com
            // and not evil.slack.com.attacker.com
            if let Some(prefix) = host.strip_suffix(suffix) {
                // prefix must end with '.' and contain no other dots
                prefix.ends_with('.') && !prefix[..prefix.len() - 1].contains('.')
            } else {
                false
            }
        } else {
            self.host.eq_ignore_ascii_case(host)
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CredentialBinding {
    pub secret_name: String,
    pub inject: CredentialInject,
    #[serde(default)]
    pub host_patterns: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CredentialInject {
    Bearer,
    Basic { username: String },
    Header { name: String, #[serde(default)] prefix: Option<String> },
    Query { param_name: String },
    UrlPath { placeholder: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimit {
    #[serde(default)]
    pub requests_per_minute: u32,
    #[serde(default)]
    pub requests_per_hour: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolInvokeCapability {
    #[serde(default)]
    pub aliases: HashMap<String, String>,
    #[serde(default = "default_max_per_execution")]
    pub max_per_execution: u32,
}

fn default_max_per_execution() -> u32 { 20 }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretsCapability {
    #[serde(default)]
    pub allowed_patterns: Vec<String>,
}

impl SecretsCapability {
    /// Check if a secret name matches any allowed pattern
    pub fn is_allowed(&self, name: &str) -> bool {
        self.allowed_patterns.iter().any(|pattern| {
            if let Some(prefix) = pattern.strip_suffix('*') {
                name.starts_with(prefix)
            } else {
                pattern == name
            }
        })
    }
}
```

**Step 5: Run tests**

Run: `cd core && cargo test --features plugin-wasm wasm::capabilities`
Expected: All 4 tests PASS

**Step 6: Commit**

```bash
git add core/src/extension/runtime/wasm/capabilities.rs core/src/extension/runtime/wasm/mod.rs
git commit -m "feat(wasm): add WasmCapabilities types with default-deny model"
```

---

## Task 3: Create `LeakDetector`

**Files:**
- Create: `core/src/exec/leak_detector.rs`
- Modify: `core/src/exec/mod.rs` (add `pub mod leak_detector;`)
- Ref: `core/src/exec/masker.rs` (reuse patterns)

**Step 1: Write the failing test**

Create `core/src/exec/leak_detector.rs` with test module:

```rust
//! Bidirectional leak detection for WASM plugin boundaries.
//!
//! Uses Aho-Corasick for fast prefix scanning + regex for full pattern matching.
//! Scans both outbound (request) and inbound (response) content.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detects_openai_key_in_outbound() {
        let detector = LeakDetector::default_patterns();
        let result = detector.scan_outbound("Authorization: Bearer sk-abc123def456ghi789jklmnopqrstuvwx");
        assert!(result.has_blocks());
    }

    #[test]
    fn test_detects_github_token() {
        let detector = LeakDetector::default_patterns();
        let result = detector.scan_outbound("token=ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefgh1234");
        assert!(result.has_blocks());
    }

    #[test]
    fn test_clean_content_passes() {
        let detector = LeakDetector::default_patterns();
        let result = detector.scan_outbound("Hello world, this is normal text");
        assert!(!result.has_blocks());
        assert!(!result.has_warnings());
    }

    #[test]
    fn test_detects_aws_key() {
        let detector = LeakDetector::default_patterns();
        let result = detector.scan_outbound("key=AKIAIOSFODNN7EXAMPLE");
        assert!(result.has_blocks());
    }

    #[test]
    fn test_detects_private_key_block() {
        let detector = LeakDetector::default_patterns();
        let result = detector.scan_outbound("-----BEGIN RSA PRIVATE KEY-----\nMIIE...");
        assert!(result.has_blocks());
    }

    #[test]
    fn test_scan_inbound_also_works() {
        let detector = LeakDetector::default_patterns();
        let result = detector.scan_inbound("response contains sk-ant-api03-secret123456789012345");
        assert!(result.has_blocks());
    }
}
```

**Step 2: Add module declaration**

In `core/src/exec/mod.rs`, add: `pub mod leak_detector;`

**Step 3: Run test to verify it fails**

Run: `cd core && cargo test exec::leak_detector`
Expected: FAIL — types not defined

**Step 4: Write implementation**

```rust
use aho_corasick::AhoCorasick;
use regex::Regex;

/// Action to take when a leak is detected
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeakAction {
    /// Abort the request entirely
    Block,
    /// Redact the content but allow continuation
    Redact,
    /// Log a warning only
    Warn,
}

/// A single leak detection pattern
#[derive(Debug, Clone)]
pub struct LeakPattern {
    pub name: &'static str,
    pub regex: Regex,
    pub action: LeakAction,
}

/// Result of a leak scan
#[derive(Debug, Default)]
pub struct ScanResult {
    pub findings: Vec<ScanFinding>,
}

#[derive(Debug)]
pub struct ScanFinding {
    pub pattern_name: &'static str,
    pub action: LeakAction,
    pub matched_text: String,
}

impl ScanResult {
    pub fn has_blocks(&self) -> bool {
        self.findings.iter().any(|f| f.action == LeakAction::Block)
    }

    pub fn has_warnings(&self) -> bool {
        self.findings.iter().any(|f| f.action == LeakAction::Warn)
    }

    pub fn is_clean(&self) -> bool {
        self.findings.is_empty()
    }
}

/// Bidirectional leak detector using Aho-Corasick + regex
pub struct LeakDetector {
    /// Fast prefix scanner for initial filtering
    ac: AhoCorasick,
    /// Prefix strings corresponding to AC patterns (same order)
    prefixes: Vec<&'static str>,
    /// Full regex patterns for confirmation
    patterns: Vec<LeakPattern>,
}

impl LeakDetector {
    /// Create detector with default patterns (matching SecretMasker + IronClaw patterns)
    pub fn default_patterns() -> Self {
        let patterns = vec![
            LeakPattern {
                name: "openai_key",
                regex: Regex::new(r"sk-[a-zA-Z0-9]{20,}").unwrap(),
                action: LeakAction::Block,
            },
            LeakPattern {
                name: "anthropic_key",
                regex: Regex::new(r"sk-ant-[a-zA-Z0-9\-]{20,}").unwrap(),
                action: LeakAction::Block,
            },
            LeakPattern {
                name: "google_api_key",
                regex: Regex::new(r"AIza[a-zA-Z0-9_\-]{35}").unwrap(),
                action: LeakAction::Block,
            },
            LeakPattern {
                name: "aws_access_key",
                regex: Regex::new(r"AKIA[A-Z0-9]{16}").unwrap(),
                action: LeakAction::Block,
            },
            LeakPattern {
                name: "github_token",
                regex: Regex::new(r"gh[pousr]_[a-zA-Z0-9]{36,}").unwrap(),
                action: LeakAction::Block,
            },
            LeakPattern {
                name: "slack_token",
                regex: Regex::new(r"xox[baprs]-[a-zA-Z0-9\-]{10,}").unwrap(),
                action: LeakAction::Block,
            },
            LeakPattern {
                name: "private_key",
                regex: Regex::new(r"-----BEGIN[A-Z ]*PRIVATE KEY-----").unwrap(),
                action: LeakAction::Block,
            },
            LeakPattern {
                name: "bearer_token",
                regex: Regex::new(r"(?i)bearer\s+[a-zA-Z0-9\-._~+/]+=*").unwrap(),
                action: LeakAction::Redact,
            },
        ];

        // Build Aho-Corasick from short prefixes for fast scanning
        let prefixes: Vec<&'static str> = vec![
            "sk-", "AIza", "AKIA", "ghp_", "gho_", "ghu_", "ghs_", "ghr_",
            "xoxb-", "xoxa-", "xoxp-", "xoxr-", "xoxs-",
            "-----BEGIN", "bearer ", "Bearer ",
        ];

        let ac = AhoCorasick::new(&prefixes).expect("valid patterns");

        Self { ac, prefixes, patterns }
    }

    /// Scan outbound content (URL + headers + body)
    pub fn scan_outbound(&self, content: &str) -> ScanResult {
        self.scan(content)
    }

    /// Scan inbound content (response body)
    pub fn scan_inbound(&self, content: &str) -> ScanResult {
        self.scan(content)
    }

    fn scan(&self, content: &str) -> ScanResult {
        let mut result = ScanResult::default();

        // Fast path: if Aho-Corasick finds no prefixes, skip regex
        if !self.ac.is_match(content) {
            return result;
        }

        // Slow path: check each regex pattern
        for pattern in &self.patterns {
            if let Some(m) = pattern.regex.find(content) {
                result.findings.push(ScanFinding {
                    pattern_name: pattern.name,
                    action: pattern.action,
                    matched_text: m.as_str()[..m.as_str().len().min(20)].to_string(),
                });
            }
        }

        result
    }
}
```

**Step 5: Run tests**

Run: `cd core && cargo test exec::leak_detector`
Expected: All 6 tests PASS

**Step 6: Commit**

```bash
git add core/src/exec/leak_detector.rs core/src/exec/mod.rs
git commit -m "feat(exec): add LeakDetector with Aho-Corasick bidirectional scanning"
```

---

## Task 4: Create `AllowlistValidator`

**Files:**
- Create: `core/src/extension/runtime/wasm/allowlist.rs`
- Modify: `core/src/extension/runtime/wasm/mod.rs` (add `mod allowlist;`)

**Step 1: Write the failing test**

Create `core/src/extension/runtime/wasm/allowlist.rs`:

```rust
//! HTTP endpoint allowlist validator with anti-bypass measures.
//!
//! Security checks:
//! - HTTPS-only enforcement
//! - Userinfo rejection (anti host-confusion)
//! - Percent-encoding normalization (anti traversal)
//! - Path traversal blocking

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::runtime::wasm::capabilities::EndpointPattern;

    fn slack_allowlist() -> AllowlistValidator {
        AllowlistValidator::new(vec![
            EndpointPattern {
                host: "slack.com".to_string(),
                path_prefix: "/api/".to_string(),
                methods: vec!["GET".to_string(), "POST".to_string()],
            },
            EndpointPattern {
                host: "*.slack.com".to_string(),
                path_prefix: "/".to_string(),
                methods: vec!["GET".to_string()],
            },
        ])
    }

    #[test]
    fn test_allows_valid_request() {
        let v = slack_allowlist();
        assert!(v.check("GET", "https://slack.com/api/users.list").is_ok());
    }

    #[test]
    fn test_rejects_http() {
        let v = slack_allowlist();
        let err = v.check("GET", "http://slack.com/api/users.list").unwrap_err();
        assert!(matches!(err, AllowlistError::HttpsRequired));
    }

    #[test]
    fn test_rejects_userinfo() {
        let v = slack_allowlist();
        let err = v.check("GET", "https://slack.com@evil.com/api/steal").unwrap_err();
        assert!(matches!(err, AllowlistError::InvalidUrl(_)));
    }

    #[test]
    fn test_rejects_unlisted_host() {
        let v = slack_allowlist();
        let err = v.check("GET", "https://evil.com/api/users.list").unwrap_err();
        assert!(matches!(err, AllowlistError::NotAllowed(_)));
    }

    #[test]
    fn test_rejects_unlisted_method() {
        let v = slack_allowlist();
        let err = v.check("DELETE", "https://slack.com/api/users.list").unwrap_err();
        assert!(matches!(err, AllowlistError::NotAllowed(_)));
    }

    #[test]
    fn test_rejects_path_traversal() {
        let v = slack_allowlist();
        let err = v.check("GET", "https://slack.com/api/../etc/passwd").unwrap_err();
        assert!(matches!(err, AllowlistError::PathTraversal));
    }
}
```

**Step 2: Add module declaration**

In `core/src/extension/runtime/wasm/mod.rs`, add: `mod allowlist;`

**Step 3: Run test to verify it fails**

Run: `cd core && cargo test --features plugin-wasm wasm::allowlist`
Expected: FAIL

**Step 4: Write implementation**

```rust
use url::Url;

use crate::extension::runtime::wasm::capabilities::EndpointPattern;

#[derive(Debug)]
pub enum AllowlistError {
    HttpsRequired,
    InvalidUrl(String),
    PathTraversal,
    NotAllowed(String),
}

impl std::fmt::Display for AllowlistError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HttpsRequired => write!(f, "HTTPS required"),
            Self::InvalidUrl(msg) => write!(f, "Invalid URL: {}", msg),
            Self::PathTraversal => write!(f, "Path traversal detected"),
            Self::NotAllowed(msg) => write!(f, "Not allowed: {}", msg),
        }
    }
}

pub struct AllowlistValidator {
    patterns: Vec<EndpointPattern>,
}

impl AllowlistValidator {
    pub fn new(patterns: Vec<EndpointPattern>) -> Self {
        Self { patterns }
    }

    /// Validate URL against allowlist
    pub fn check(&self, method: &str, url_str: &str) -> Result<(), AllowlistError> {
        let parsed = Url::parse(url_str)
            .map_err(|e| AllowlistError::InvalidUrl(e.to_string()))?;

        // HTTPS only
        if parsed.scheme() != "https" {
            return Err(AllowlistError::HttpsRequired);
        }

        // Reject userinfo (anti host-confusion: https://victim@attacker.com)
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err(AllowlistError::InvalidUrl(
                "URL contains userinfo (@) which is not allowed".to_string(),
            ));
        }

        let host = parsed.host_str()
            .ok_or_else(|| AllowlistError::InvalidUrl("no host".to_string()))?;

        let path = parsed.path();

        // Path traversal check
        if path.contains("..") {
            return Err(AllowlistError::PathTraversal);
        }

        // Check percent-encoded traversal (%2e%2e)
        let decoded_path = percent_encoding::percent_decode_str(path)
            .decode_utf8_lossy();
        if decoded_path.contains("..") {
            return Err(AllowlistError::PathTraversal);
        }

        // Match against allowlist
        for pattern in &self.patterns {
            if pattern.matches(method, host, path) {
                return Ok(());
            }
        }

        Err(AllowlistError::NotAllowed(format!(
            "{} {} not in allowlist", method, host
        )))
    }
}
```

Note: This requires the `url` crate. Check if already in Cargo.toml — yes, `url = "2.5"` is a dependency of reqwest which is already included. Add `url = "2.5"` as a direct dependency if not present, and `percent-encoding = "2.3"`.

**Step 5: Run tests**

Run: `cd core && cargo test --features plugin-wasm wasm::allowlist`
Expected: All 6 tests PASS

**Step 6: Commit**

```bash
git add core/src/extension/runtime/wasm/allowlist.rs core/src/extension/runtime/wasm/mod.rs core/Cargo.toml
git commit -m "feat(wasm): add AllowlistValidator with anti-bypass security"
```

---

## Task 5: Create `CredentialInjector`

**Files:**
- Create: `core/src/extension/runtime/wasm/credential_injector.rs`
- Modify: `core/src/extension/runtime/wasm/mod.rs` (add `mod credential_injector;`)

**Step 1: Write the failing test**

```rust
//! Credential injection at the host boundary.
//!
//! WASM plugins never see secret values. The host resolves credentials
//! from SecretStore and injects them into HTTP requests.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::runtime::wasm::capabilities::{CredentialBinding, CredentialInject};

    #[test]
    fn test_bearer_injection() {
        let mut headers = Vec::new();
        let binding = CredentialBinding {
            secret_name: "slack_token".to_string(),
            inject: CredentialInject::Bearer,
            host_patterns: vec!["slack.com".to_string()],
        };

        let url = "https://slack.com/api/test";
        let secrets = vec![("slack_token".to_string(), "xoxb-secret-value".to_string())];
        let result = inject_credential(&binding, url, &mut headers, &secrets);

        assert!(result.is_ok());
        assert!(headers.iter().any(|(k, v)| k == "Authorization" && v == "Bearer xoxb-secret-value"));
    }

    #[test]
    fn test_header_injection_with_prefix() {
        let mut headers = Vec::new();
        let binding = CredentialBinding {
            secret_name: "api_key".to_string(),
            inject: CredentialInject::Header {
                name: "X-API-Key".to_string(),
                prefix: Some("Key ".to_string()),
            },
            host_patterns: vec!["api.example.com".to_string()],
        };

        let url = "https://api.example.com/v1/data";
        let secrets = vec![("api_key".to_string(), "my-secret-key".to_string())];
        let result = inject_credential(&binding, url, &mut headers, &secrets);

        assert!(result.is_ok());
        assert!(headers.iter().any(|(k, v)| k == "X-API-Key" && v == "Key my-secret-key"));
    }

    #[test]
    fn test_query_injection() {
        let mut headers = Vec::new();
        let binding = CredentialBinding {
            secret_name: "api_key".to_string(),
            inject: CredentialInject::Query { param_name: "key".to_string() },
            host_patterns: vec!["maps.googleapis.com".to_string()],
        };

        let url = "https://maps.googleapis.com/api/geocode";
        let secrets = vec![("api_key".to_string(), "AIza-test-key".to_string())];
        let result = inject_credential(&binding, url, &mut headers, &secrets);

        assert!(result.is_ok());
        let new_url = result.unwrap();
        assert!(new_url.is_some()); // URL was modified
        assert!(new_url.unwrap().contains("key=AIza-test-key"));
    }

    #[test]
    fn test_host_pattern_mismatch_skips() {
        let mut headers = Vec::new();
        let binding = CredentialBinding {
            secret_name: "token".to_string(),
            inject: CredentialInject::Bearer,
            host_patterns: vec!["slack.com".to_string()],
        };

        let url = "https://evil.com/steal";
        let secrets = vec![("token".to_string(), "secret".to_string())];
        let result = inject_credential(&binding, url, &mut headers, &secrets);

        assert!(result.is_ok());
        assert!(headers.is_empty()); // No injection — host didn't match
    }

    #[test]
    fn test_missing_secret_errors() {
        let mut headers = Vec::new();
        let binding = CredentialBinding {
            secret_name: "nonexistent".to_string(),
            inject: CredentialInject::Bearer,
            host_patterns: vec!["slack.com".to_string()],
        };

        let url = "https://slack.com/api/test";
        let secrets: Vec<(String, String)> = vec![];
        let result = inject_credential(&binding, url, &mut headers, &secrets);

        assert!(result.is_err());
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cd core && cargo test --features plugin-wasm wasm::credential_injector`
Expected: FAIL

**Step 3: Write implementation**

Implement `inject_credential()` function that:
- Parses URL to extract host
- Matches against `host_patterns` (with `*` wildcard support)
- Looks up secret value from the provided secrets slice
- Injects credential based on `CredentialInject` variant:
  - `Bearer` → add `Authorization: Bearer <value>` header
  - `Basic` → add `Authorization: Basic base64(username:value)` header
  - `Header` → add `<name>: <prefix><value>` header
  - `Query` → append `?param_name=value` to URL (return modified URL)
  - `UrlPath` → replace placeholder in URL (return modified URL)

**Step 4: Run tests**

Run: `cd core && cargo test --features plugin-wasm wasm::credential_injector`
Expected: All 5 tests PASS

**Step 5: Commit**

```bash
git add core/src/extension/runtime/wasm/credential_injector.rs core/src/extension/runtime/wasm/mod.rs
git commit -m "feat(wasm): add CredentialInjector — plugins never see secrets"
```

---

## Task 6: Create `WasmResourceLimits`

**Files:**
- Create: `core/src/extension/runtime/wasm/limits.rs`
- Modify: `core/src/extension/runtime/wasm/mod.rs` (add `mod limits;`)

**Step 1: Write the failing test**

```rust
//! Resource limits for WASM plugin execution.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_limits() {
        let limits = WasmResourceLimits::default();
        assert_eq!(limits.memory_mb, 10);
        assert_eq!(limits.fuel, 10_000_000);
        assert_eq!(limits.timeout_secs, 60);
        assert_eq!(limits.max_http_calls, 50);
        assert_eq!(limits.max_tool_invokes, 20);
        assert_eq!(limits.max_log_entries, 1000);
        assert_eq!(limits.max_log_message_bytes, 4096);
    }

    #[test]
    fn test_custom_limits() {
        let limits = WasmResourceLimits {
            memory_mb: 20,
            fuel: 5_000_000,
            ..Default::default()
        };
        assert_eq!(limits.memory_mb, 20);
        assert_eq!(limits.fuel, 5_000_000);
        assert_eq!(limits.timeout_secs, 60); // still default
    }
}
```

**Step 2: Write implementation**

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmResourceLimits {
    pub memory_mb: u32,
    pub fuel: u64,
    pub timeout_secs: u64,
    pub max_http_calls: u32,
    pub max_tool_invokes: u32,
    pub max_log_entries: u32,
    pub max_log_message_bytes: usize,
}

impl Default for WasmResourceLimits {
    fn default() -> Self {
        Self {
            memory_mb: 10,
            fuel: 10_000_000,
            timeout_secs: 60,
            max_http_calls: 50,
            max_tool_invokes: 20,
            max_log_entries: 1000,
            max_log_message_bytes: 4096,
        }
    }
}
```

**Step 3: Run tests**

Run: `cd core && cargo test --features plugin-wasm wasm::limits`
Expected: Both tests PASS

**Step 4: Commit**

```bash
git add core/src/extension/runtime/wasm/limits.rs core/src/extension/runtime/wasm/mod.rs
git commit -m "feat(wasm): add WasmResourceLimits with sensible defaults"
```

---

## Task 7: Create `CapabilityError` and `WasmCapabilityKernel`

**Files:**
- Create: `core/src/extension/runtime/wasm/capability_kernel.rs`
- Modify: `core/src/extension/runtime/wasm/mod.rs` (add `mod capability_kernel;`)
- Ref: `core/src/exec/leak_detector.rs`, `core/src/exec/sandbox/audit.rs`

This is the largest task. The kernel is the central security component.

**Step 1: Write the failing test**

```rust
//! WasmCapabilityKernel — per-execution security kernel.
//!
//! Every host function call passes through this kernel for:
//! - Capability checking (default-deny)
//! - Leak detection (bidirectional)
//! - Credential injection (host-side)
//! - Audit logging
//! - Resource counting

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::runtime::wasm::capabilities::*;

    fn kernel_with_no_caps() -> WasmCapabilityKernel {
        WasmCapabilityKernel::new(
            "test-plugin".to_string(),
            WasmCapabilities::default(),
            WasmResourceLimits::default(),
        )
    }

    fn kernel_with_workspace() -> WasmCapabilityKernel {
        let mut caps = WasmCapabilities::default();
        caps.workspace = Some(WorkspaceCapability {
            allowed_prefixes: vec!["docs/".to_string(), "config/".to_string()],
        });
        WasmCapabilityKernel::new(
            "test-plugin".to_string(),
            caps,
            WasmResourceLimits::default(),
        )
    }

    fn kernel_with_secrets() -> WasmCapabilityKernel {
        let mut caps = WasmCapabilities::default();
        caps.secrets = Some(SecretsCapability {
            allowed_patterns: vec!["slack_*".to_string()],
        });
        WasmCapabilityKernel::new(
            "test-plugin".to_string(),
            caps,
            WasmResourceLimits::default(),
        )
    }

    #[test]
    fn test_no_workspace_capability_denies_read() {
        let kernel = kernel_with_no_caps();
        let result = kernel.check_workspace_read("any/path");
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), CapabilityError::NotDeclared(_)));
    }

    #[test]
    fn test_workspace_allowed_prefix() {
        let kernel = kernel_with_workspace();
        assert!(kernel.check_workspace_read("docs/readme.md").is_ok());
        assert!(kernel.check_workspace_read("config/app.toml").is_ok());
    }

    #[test]
    fn test_workspace_rejects_outside_prefix() {
        let kernel = kernel_with_workspace();
        let result = kernel.check_workspace_read("secrets/key.pem");
        assert!(result.is_err());
    }

    #[test]
    fn test_workspace_rejects_path_traversal() {
        let kernel = kernel_with_workspace();
        assert!(kernel.check_workspace_read("docs/../secrets/key.pem").is_err());
        assert!(kernel.check_workspace_read("/etc/passwd").is_err());
        assert!(kernel.check_workspace_read("docs/\0hidden").is_err());
    }

    #[test]
    fn test_secret_exists_with_capability() {
        let kernel = kernel_with_secrets();
        // We can only check the capability gate, not actual secret lookup
        // (that would need a SecretStore mock)
        assert!(kernel.check_secret_pattern("slack_bot_token"));
        assert!(!kernel.check_secret_pattern("aws_key"));
    }

    #[test]
    fn test_secret_exists_without_capability_denies_all() {
        let kernel = kernel_with_no_caps();
        assert!(!kernel.check_secret_pattern("anything"));
    }

    #[test]
    fn test_log_respects_limits() {
        let limits = WasmResourceLimits {
            max_log_entries: 2,
            ..Default::default()
        };
        let kernel = WasmCapabilityKernel::new(
            "test".to_string(),
            WasmCapabilities::default(),
            limits,
        );
        assert!(kernel.log("info", "first").is_ok());
        assert!(kernel.log("info", "second").is_ok());
        assert!(kernel.log("info", "third").is_err()); // limit exceeded
    }

    #[test]
    fn test_log_truncates_long_messages() {
        let limits = WasmResourceLimits {
            max_log_message_bytes: 10,
            ..Default::default()
        };
        let kernel = WasmCapabilityKernel::new(
            "test".to_string(),
            WasmCapabilities::default(),
            limits,
        );
        // Long message should be accepted but truncated (kernel logs it)
        assert!(kernel.log("info", "this is a very long message").is_ok());
    }

    #[test]
    fn test_now_millis_returns_reasonable_value() {
        let kernel = kernel_with_no_caps();
        let ts = kernel.now_millis();
        // Should be after 2026-01-01 and before 2030-01-01
        assert!(ts > 1_767_225_600_000);
        assert!(ts < 1_893_456_000_000);
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cd core && cargo test --features plugin-wasm wasm::capability_kernel`
Expected: FAIL

**Step 3: Write implementation**

```rust
use std::sync::atomic::{AtomicU32, Ordering};

use crate::extension::runtime::wasm::capabilities::*;
use crate::extension::runtime::wasm::limits::WasmResourceLimits;

/// Errors from capability checks
#[derive(Debug)]
pub enum CapabilityError {
    NotDeclared(String),
    NotAllowed(String),
    RateLimited(String),
    ResourceExhausted(String),
    LeakDetected(String),
    PathTraversal(String),
    InternalError(String),
}

impl std::fmt::Display for CapabilityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotDeclared(msg) => write!(f, "Capability not declared: {}", msg),
            Self::NotAllowed(msg) => write!(f, "Not allowed: {}", msg),
            Self::RateLimited(msg) => write!(f, "Rate limited: {}", msg),
            Self::ResourceExhausted(msg) => write!(f, "Resource exhausted: {}", msg),
            Self::LeakDetected(msg) => write!(f, "Leak detected: {}", msg),
            Self::PathTraversal(msg) => write!(f, "Path traversal: {}", msg),
            Self::InternalError(msg) => write!(f, "Internal error: {}", msg),
        }
    }
}

impl std::error::Error for CapabilityError {}

/// Per-execution security kernel for WASM plugins
pub struct WasmCapabilityKernel {
    plugin_id: String,
    capabilities: WasmCapabilities,
    limits: WasmResourceLimits,

    // Per-execution counters
    log_count: AtomicU32,
    http_call_count: AtomicU32,
    tool_invoke_count: AtomicU32,
}

impl WasmCapabilityKernel {
    pub fn new(
        plugin_id: String,
        capabilities: WasmCapabilities,
        limits: WasmResourceLimits,
    ) -> Self {
        Self {
            plugin_id,
            capabilities,
            limits,
            log_count: AtomicU32::new(0),
            http_call_count: AtomicU32::new(0),
            tool_invoke_count: AtomicU32::new(0),
        }
    }

    /// Check workspace read permission
    pub fn check_workspace_read(&self, path: &str) -> Result<(), CapabilityError> {
        let ws = self.capabilities.workspace.as_ref().ok_or_else(|| {
            CapabilityError::NotDeclared("workspace".to_string())
        })?;

        // Security checks
        self.validate_path(path)?;

        // Prefix check
        if !ws.allowed_prefixes.is_empty()
            && !ws.allowed_prefixes.iter().any(|p| path.starts_with(p))
        {
            return Err(CapabilityError::NotAllowed(format!(
                "path '{}' not in allowed prefixes", path
            )));
        }

        Ok(())
    }

    /// Check if secret name matches declared patterns (does not access store)
    pub fn check_secret_pattern(&self, name: &str) -> bool {
        self.capabilities
            .secrets
            .as_ref()
            .map(|s| s.is_allowed(name))
            .unwrap_or(false)
    }

    /// Log a message (respects limits)
    pub fn log(&self, _level: &str, msg: &str) -> Result<(), CapabilityError> {
        let count = self.log_count.fetch_add(1, Ordering::Relaxed);
        if count >= self.limits.max_log_entries {
            return Err(CapabilityError::ResourceExhausted(
                "log entry limit exceeded".to_string(),
            ));
        }

        // Truncate if needed (just log it, don't error)
        let _msg = if msg.len() > self.limits.max_log_message_bytes {
            &msg[..self.limits.max_log_message_bytes]
        } else {
            msg
        };

        // Actual logging would go through tracing
        Ok(())
    }

    /// Get current timestamp in milliseconds
    pub fn now_millis(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }

    /// Increment and check HTTP call counter
    pub fn check_http_limit(&self) -> Result<(), CapabilityError> {
        let count = self.http_call_count.fetch_add(1, Ordering::Relaxed);
        if count >= self.limits.max_http_calls {
            return Err(CapabilityError::ResourceExhausted(
                "HTTP call limit exceeded".to_string(),
            ));
        }
        Ok(())
    }

    /// Increment and check tool invoke counter
    pub fn check_tool_invoke_limit(&self) -> Result<(), CapabilityError> {
        let count = self.tool_invoke_count.fetch_add(1, Ordering::Relaxed);
        if count >= self.limits.max_tool_invokes {
            return Err(CapabilityError::ResourceExhausted(
                "tool invoke limit exceeded".to_string(),
            ));
        }
        Ok(())
    }

    /// Resolve tool alias to real name
    pub fn resolve_tool_alias(&self, alias: &str) -> Result<String, CapabilityError> {
        let ti = self.capabilities.tool_invoke.as_ref().ok_or_else(|| {
            CapabilityError::NotDeclared("tool_invoke".to_string())
        })?;

        ti.aliases.get(alias).cloned().ok_or_else(|| {
            CapabilityError::NotAllowed(format!("unknown tool alias: {}", alias))
        })
    }

    pub fn plugin_id(&self) -> &str {
        &self.plugin_id
    }

    pub fn capabilities(&self) -> &WasmCapabilities {
        &self.capabilities
    }

    // -- Private helpers --

    fn validate_path(&self, path: &str) -> Result<(), CapabilityError> {
        if path.contains("..") {
            return Err(CapabilityError::PathTraversal(
                "'..' not allowed".to_string(),
            ));
        }
        if path.starts_with('/') {
            return Err(CapabilityError::PathTraversal(
                "absolute paths not allowed".to_string(),
            ));
        }
        if path.contains('\0') {
            return Err(CapabilityError::PathTraversal(
                "null bytes not allowed".to_string(),
            ));
        }
        Ok(())
    }
}
```

**Step 4: Run tests**

Run: `cd core && cargo test --features plugin-wasm wasm::capability_kernel`
Expected: All 9 tests PASS

**Step 5: Commit**

```bash
git add core/src/extension/runtime/wasm/capability_kernel.rs core/src/extension/runtime/wasm/mod.rs
git commit -m "feat(wasm): add WasmCapabilityKernel — per-execution security enforcement"
```

---

## Task 8: Create Host Functions and Wire into Extism

**Files:**
- Create: `core/src/extension/runtime/wasm/host_functions.rs`
- Modify: `core/src/extension/runtime/wasm/mod.rs` (major changes to `load_plugin` and `call_tool`)

This task registers the 6 host functions with Extism and rewires the plugin loading path.

**Step 1: Write the host functions module**

```rust
//! Extism host function registrations for WASM plugins.
//!
//! Registers 6 host functions:
//! - log(level, message)
//! - now_millis() -> u64
//! - workspace_read(path) -> Option<string>
//! - http_request(...) -> Result<response, error>
//! - tool_invoke(alias, params) -> Result<string, error>
//! - secret_exists(name) -> bool

#[cfg(feature = "plugin-wasm")]
use std::sync::Arc;

#[cfg(feature = "plugin-wasm")]
use extism::{host_fn, UserData};

#[cfg(feature = "plugin-wasm")]
use crate::extension::runtime::wasm::capability_kernel::WasmCapabilityKernel;

/// Shared state passed to all host functions via Extism UserData
#[cfg(feature = "plugin-wasm")]
pub struct HostState {
    pub kernel: Arc<WasmCapabilityKernel>,
    pub workspace_root: std::path::PathBuf,
}

// -- Host function implementations --
// Each function receives HostState via UserData and delegates to the kernel.

#[cfg(feature = "plugin-wasm")]
host_fn!(pub host_log(state: HostState; level: String, message: String) {
    let state = state.get()?;
    let state = state.lock().unwrap();
    let _ = state.kernel.log(&level, &message);
    Ok(())
});

#[cfg(feature = "plugin-wasm")]
host_fn!(pub host_now_millis(state: HostState;) -> u64 {
    let state = state.get()?;
    let state = state.lock().unwrap();
    Ok(state.kernel.now_millis())
});

#[cfg(feature = "plugin-wasm")]
host_fn!(pub host_workspace_read(state: HostState; path: String) -> String {
    let state = state.get()?;
    let state = state.lock().unwrap();

    // Check capability
    if let Err(e) = state.kernel.check_workspace_read(&path) {
        return Ok(serde_json::json!({"error": e.to_string()}).to_string());
    }

    // Read file from workspace
    let full_path = state.workspace_root.join(&path);
    match std::fs::read_to_string(&full_path) {
        Ok(content) => Ok(serde_json::json!({"content": content}).to_string()),
        Err(e) => Ok(serde_json::json!({"error": e.to_string()}).to_string()),
    }
});

#[cfg(feature = "plugin-wasm")]
host_fn!(pub host_secret_exists(state: HostState; name: String) -> String {
    let state = state.get()?;
    let state = state.lock().unwrap();
    let exists = state.kernel.check_secret_pattern(&name);
    Ok(exists.to_string())
});
```

Note: `http_request` and `tool_invoke` are more complex (async). For Extism host functions which are sync, we need to use `tokio::runtime::Handle::current().block_on()` or store a runtime handle. These will be implemented with a sync wrapper over the async operations.

**Step 2: Rewire `mod.rs` — `load_plugin` to use `PluginBuilder`**

Modify `core/src/extension/runtime/wasm/mod.rs`:

- Change `Plugin::new(&extism_manifest, [], true)` to use `PluginBuilder` with host functions
- Add `WasmCapabilityKernel` to `LoadedWasmPlugin`
- In `call_tool`, delegate through kernel

Key change in `load_plugin` (line 64-90):

```rust
#[cfg(feature = "plugin-wasm")]
pub fn load_plugin(&mut self, manifest: &PluginManifest) -> Result<(), ExtensionError> {
    use extism::{PluginBuilder, Wasm, PTR};

    let wasm_path = manifest.entry_path();
    if !wasm_path.exists() {
        return Err(ExtensionError::Runtime(format!(
            "WASM file not found: {:?}", wasm_path
        )));
    }

    info!("Loading WASM plugin: {} from {:?}", manifest.id, wasm_path);

    // Parse capabilities from manifest (default = zero permissions)
    let capabilities = manifest.wasm_capabilities.clone().unwrap_or_default();
    let limits = manifest.wasm_resource_limits.clone().unwrap_or_default();

    // Create per-plugin kernel
    let kernel = Arc::new(WasmCapabilityKernel::new(
        manifest.id.clone(),
        capabilities,
        limits,
    ));

    // Create host state
    let host_state = UserData::new(HostState {
        kernel: kernel.clone(),
        workspace_root: manifest.root_dir.clone(),
    });

    let extism_manifest = ExtismManifest::new([Wasm::file(&wasm_path)]);

    let plugin = PluginBuilder::new(extism_manifest)
        .with_wasi(true)
        .with_function("log", [PTR, PTR], [], host_state.clone(), host_log)
        .with_function("now_millis", [], [PTR], host_state.clone(), host_now_millis)
        .with_function("workspace_read", [PTR], [PTR], host_state.clone(), host_workspace_read)
        .with_function("secret_exists", [PTR], [PTR], host_state.clone(), host_secret_exists)
        .build()
        .map_err(|e| ExtensionError::Runtime(format!("Failed to load WASM: {}", e)))?;

    let loaded = LoadedWasmPlugin {
        plugin,
        manifest: manifest.clone(),
        kernel,
    };

    self.plugins.insert(manifest.id.clone(), loaded);
    Ok(())
}
```

**Step 3: Update `LoadedWasmPlugin` struct**

Replace `permissions: PermissionChecker` with `kernel: Arc<WasmCapabilityKernel>`.

**Step 4: Test manually**

Run: `cd core && cargo check --features plugin-wasm`
Expected: Compiles (existing tests still pass since they don't load real WASM)

Run: `cd core && cargo test --features plugin-wasm`
Expected: All existing + new tests pass

**Step 5: Commit**

```bash
git add core/src/extension/runtime/wasm/
git commit -m "feat(wasm): register host functions via PluginBuilder with capability kernel"
```

---

## Task 9: Parse `[plugin.capabilities]` from TOML

**Files:**
- Modify: `core/src/extension/manifest/aleph_plugin_toml.rs:356-366` (expand `CapabilitiesSection`)
- Modify: `core/src/extension/manifest/types.rs:237-257` (add `wasm_capabilities` field)
- Modify: `core/src/extension/manifest/aleph_plugin_toml.rs:505+` (wire parsing)

**Step 1: Write the failing test**

Add to `core/src/extension/manifest/aleph_plugin_toml.rs` tests:

```rust
#[test]
fn test_parse_wasm_capabilities() {
    let content = r#"
[plugin]
id = "test-wasm"
name = "Test WASM"
kind = "wasm"
entry = "plugin.wasm"

[plugin.capabilities.workspace]
allowed_prefixes = ["docs/", "config/"]

[plugin.capabilities.http]
timeout_secs = 30

[[plugin.capabilities.http.allowlist]]
host = "api.slack.com"
path_prefix = "/api/"
methods = ["GET", "POST"]

[[plugin.capabilities.http.credentials]]
secret_name = "slack_token"
inject = { type = "bearer" }
host_patterns = ["api.slack.com"]

[plugin.capabilities.tool_invoke]
max_per_execution = 10

[plugin.capabilities.tool_invoke.aliases]
search = "brave_search"

[plugin.capabilities.secrets]
allowed_patterns = ["slack_*"]
"#;

    let manifest = parse_aleph_plugin_toml_content(content, Path::new("/tmp/test")).unwrap();
    let caps = manifest.wasm_capabilities.as_ref().unwrap();
    assert!(caps.workspace.is_some());
    assert!(caps.http.is_some());
    assert_eq!(caps.http.as_ref().unwrap().allowlist.len(), 1);
    assert_eq!(caps.http.as_ref().unwrap().credentials.len(), 1);
    assert!(caps.tool_invoke.is_some());
    assert_eq!(caps.tool_invoke.as_ref().unwrap().aliases.len(), 1);
    assert!(caps.secrets.is_some());
}

#[test]
fn test_parse_no_capabilities_gives_none() {
    let content = r#"
[plugin]
id = "simple"
name = "Simple"
kind = "wasm"
entry = "plugin.wasm"
"#;
    let manifest = parse_aleph_plugin_toml_content(content, Path::new("/tmp/test")).unwrap();
    assert!(manifest.wasm_capabilities.is_none());
}
```

**Step 2: Expand `CapabilitiesSection` (line 358)**

Replace the existing `CapabilitiesSection` with a richer version that can hold both the old fields and the new WASM capability fields:

```rust
/// Advanced capabilities section
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CapabilitiesSection {
    /// Plugin can dynamically register tools at runtime
    #[serde(default)]
    pub dynamic_tools: bool,

    /// Plugin can dynamically register hooks at runtime
    #[serde(default)]
    pub dynamic_hooks: bool,

    // WASM sandbox capabilities (new)
    /// Workspace read access
    #[serde(default)]
    pub workspace: Option<WasmWorkspaceToml>,

    /// HTTP access control
    #[serde(default)]
    pub http: Option<WasmHttpToml>,

    /// Tool invocation via aliases
    #[serde(default)]
    pub tool_invoke: Option<WasmToolInvokeToml>,

    /// Secret existence checking
    #[serde(default)]
    pub secrets: Option<WasmSecretsToml>,
}
```

Add the TOML struct definitions (serde-tagged to match the TOML format):

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmWorkspaceToml {
    #[serde(default)]
    pub allowed_prefixes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmHttpToml {
    #[serde(default)]
    pub allowlist: Vec<WasmEndpointToml>,
    #[serde(default)]
    pub credentials: Vec<WasmCredentialToml>,
    #[serde(default)]
    pub rate_limit: Option<WasmRateLimitToml>,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    #[serde(default = "default_max_req")]
    pub max_request_bytes: usize,
    #[serde(default = "default_max_resp")]
    pub max_response_bytes: usize,
}

// ... (matching TOML structures that mirror capabilities.rs types)
```

**Step 3: Add conversion from TOML types → WasmCapabilities**

Write a `convert_wasm_capabilities(caps: &CapabilitiesSection) -> Option<WasmCapabilities>` function that converts the TOML structs to the runtime `WasmCapabilities` type. Returns `None` if no WASM capabilities were declared.

**Step 4: Add `wasm_capabilities` field to `PluginManifest`**

In `types.rs`, add to the V2 fields section:

```rust
/// WASM capability configuration (parsed from [plugin.capabilities])
#[serde(skip)]
pub wasm_capabilities: Option<WasmCapabilities>,

/// WASM resource limits (parsed from [plugin.limits])
#[serde(skip)]
pub wasm_resource_limits: Option<WasmResourceLimits>,
```

**Step 5: Wire parsing in `parse_aleph_plugin_toml_content`**

After the existing `capabilities_v2` assignment, add:

```rust
manifest.wasm_capabilities = convert_wasm_capabilities(&toml.capabilities);
```

**Step 6: Run tests**

Run: `cd core && cargo test --features plugin-wasm manifest::aleph_plugin_toml`
Expected: All tests PASS (including the 2 new ones)

**Step 7: Commit**

```bash
git add core/src/extension/manifest/aleph_plugin_toml.rs core/src/extension/manifest/types.rs
git commit -m "feat(manifest): parse WASM capabilities from aleph.plugin.toml"
```

---

## Task 10: Rewrite `permissions.rs` to Use Capability Kernel

**Files:**
- Modify: `core/src/extension/runtime/wasm/permissions.rs`

The old `PermissionChecker` checked boolean flags. The new version delegates to `WasmCapabilityKernel`.

**Step 1: Simplify or deprecate**

Since capabilities are now managed by `WasmCapabilityKernel`, `PermissionChecker` can be simplified to a thin facade that checks whether a specific capability is declared:

```rust
use crate::extension::runtime::wasm::capabilities::WasmCapabilities;

/// Checks whether WASM capabilities are declared.
/// Actual enforcement is in WasmCapabilityKernel.
#[derive(Debug, Clone, Default)]
pub struct PermissionChecker {
    capabilities: Option<WasmCapabilities>,
}

impl PermissionChecker {
    pub fn new(capabilities: Option<WasmCapabilities>) -> Self {
        Self { capabilities }
    }

    pub fn has_http(&self) -> bool {
        self.capabilities.as_ref().map(|c| c.http.is_some()).unwrap_or(false)
    }

    pub fn has_workspace(&self) -> bool {
        self.capabilities.as_ref().map(|c| c.workspace.is_some()).unwrap_or(false)
    }

    pub fn has_tool_invoke(&self) -> bool {
        self.capabilities.as_ref().map(|c| c.tool_invoke.is_some()).unwrap_or(false)
    }

    pub fn has_secrets(&self) -> bool {
        self.capabilities.as_ref().map(|c| c.secrets.is_some()).unwrap_or(false)
    }
}
```

**Step 2: Run all WASM tests**

Run: `cd core && cargo test --features plugin-wasm`
Expected: All tests PASS

**Step 3: Commit**

```bash
git add core/src/extension/runtime/wasm/permissions.rs
git commit -m "refactor(wasm): simplify PermissionChecker to facade over WasmCapabilities"
```

---

## Task 11: Integration Test — End-to-End Capability Enforcement

**Files:**
- Create: `core/tests/wasm_capability_test.rs` (integration test)

**Step 1: Write integration test**

```rust
//! Integration test for WASM capability kernel end-to-end.

#[cfg(feature = "plugin-wasm")]
mod tests {
    use alephcore::extension::runtime::wasm::capabilities::*;
    use alephcore::extension::runtime::wasm::capability_kernel::*;
    use alephcore::extension::runtime::wasm::limits::WasmResourceLimits;
    use alephcore::exec::leak_detector::LeakDetector;

    #[test]
    fn test_full_capability_lifecycle() {
        // 1. Plugin with http + workspace capabilities
        let caps = WasmCapabilities {
            workspace: Some(WorkspaceCapability {
                allowed_prefixes: vec!["data/".to_string()],
            }),
            http: Some(HttpCapability {
                allowlist: vec![EndpointPattern {
                    host: "api.example.com".to_string(),
                    path_prefix: "/v1/".to_string(),
                    methods: vec!["GET".to_string()],
                }],
                credentials: vec![],
                rate_limit: None,
                timeout_secs: 30,
                max_request_bytes: 1_048_576,
                max_response_bytes: 10_485_760,
            }),
            tool_invoke: None,
            secrets: None,
        };

        let kernel = WasmCapabilityKernel::new(
            "test-plugin".to_string(),
            caps,
            WasmResourceLimits::default(),
        );

        // 2. Workspace read — allowed
        assert!(kernel.check_workspace_read("data/input.json").is_ok());

        // 3. Workspace read — denied (wrong prefix)
        assert!(kernel.check_workspace_read("secrets/key.pem").is_err());

        // 4. Workspace read — denied (traversal)
        assert!(kernel.check_workspace_read("data/../secrets/key").is_err());

        // 5. Tool invoke — denied (not declared)
        assert!(kernel.resolve_tool_alias("anything").is_err());

        // 6. Log — works (always allowed)
        assert!(kernel.log("info", "test message").is_ok());

        // 7. Clock — works (always allowed)
        assert!(kernel.now_millis() > 0);
    }

    #[test]
    fn test_leak_detector_integration() {
        let detector = LeakDetector::default_patterns();

        // Outbound with API key → blocked
        let result = detector.scan_outbound("key=sk-abcdefghijklmnopqrstuvwxyz12345");
        assert!(result.has_blocks());

        // Clean outbound → passes
        let result = detector.scan_outbound("Hello, this is clean content");
        assert!(result.is_clean());
    }
}
```

**Step 2: Run integration test**

Run: `cd core && cargo test --features plugin-wasm --test wasm_capability_test`
Expected: All tests PASS

**Step 3: Commit**

```bash
git add core/tests/wasm_capability_test.rs
git commit -m "test: add end-to-end WASM capability integration tests"
```

---

## Task 12: Final Cleanup and Documentation

**Files:**
- Modify: `core/src/extension/runtime/wasm/mod.rs` (ensure all modules are properly exported)
- Modify: `core/src/lib.rs` (ensure public API is accessible)

**Step 1: Verify all module exports**

In `core/src/extension/runtime/wasm/mod.rs`, ensure:
```rust
mod capabilities;
mod capability_kernel;
mod allowlist;
mod credential_injector;
mod host_functions;
mod limits;
mod permissions;

pub use capabilities::WasmCapabilities;
pub use capability_kernel::{WasmCapabilityKernel, CapabilityError};
pub use limits::WasmResourceLimits;
pub use permissions::PermissionChecker;
```

**Step 2: Run full test suite**

Run: `cd core && cargo test --features plugin-wasm`
Expected: All tests PASS

Run: `cd core && cargo test` (without plugin-wasm feature)
Expected: All tests PASS (feature-gated code compiles away cleanly)

Run: `cd core && cargo clippy --features plugin-wasm`
Expected: No warnings

**Step 3: Commit**

```bash
git add -A
git commit -m "cleanup: finalize WASM capability kernel module exports and docs"
```

---

## Summary

| Task | Component | Est. Files |
|------|-----------|------------|
| 1 | Add aho-corasick dep | 1 |
| 2 | WasmCapabilities types | 2 |
| 3 | LeakDetector | 2 |
| 4 | AllowlistValidator | 2 |
| 5 | CredentialInjector | 2 |
| 6 | WasmResourceLimits | 2 |
| 7 | WasmCapabilityKernel | 2 |
| 8 | Host Functions + Extism wiring | 2 |
| 9 | TOML capabilities parsing | 2 |
| 10 | Rewrite PermissionChecker | 1 |
| 11 | Integration tests | 1 |
| 12 | Cleanup + exports | 2 |

**Total: 12 tasks, ~12 new/modified files, ~1500 lines of code + tests**
