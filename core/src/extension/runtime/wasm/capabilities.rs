//! WASM plugin capability model (default-deny)
//!
//! Defines fine-grained capabilities that a WASM plugin may request.
//! All capabilities are `Option`-wrapped: `None` means the capability
//! is not granted (default-deny).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Top-level capability declaration for a WASM plugin.
///
/// Every field is `Option`: `None` = capability not granted (default-deny).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WasmCapabilities {
    pub workspace: Option<WorkspaceCapability>,
    pub http: Option<HttpCapability>,
    pub tool_invoke: Option<ToolInvokeCapability>,
    pub secrets: Option<SecretsCapability>,
}

/// Grants read/write access to workspace files under specific prefixes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceCapability {
    #[serde(default)]
    pub allowed_prefixes: Vec<String>,
}

/// Grants outbound HTTP access with allowlist, credentials, and rate limits.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpCapability {
    pub allowlist: Vec<EndpointPattern>,
    #[serde(default)]
    pub credentials: Vec<CredentialBinding>,
    pub rate_limit: Option<RateLimit>,
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
    #[serde(default = "default_max_request_bytes")]
    pub max_request_bytes: usize,
    #[serde(default = "default_max_response_bytes")]
    pub max_response_bytes: usize,
}

fn default_timeout_secs() -> u64 {
    30
}

fn default_max_request_bytes() -> usize {
    1_048_576 // 1 MB
}

fn default_max_response_bytes() -> usize {
    10_485_760 // 10 MB
}

/// A pattern that matches HTTP requests by host, path prefix, and methods.
///
/// The `host` field supports leading wildcard notation (`*.domain.com`).
/// A wildcard `*.domain.com` matches `api.domain.com` but does NOT match
/// `domain.com` (no subdomain) or `evil.domain.com.attacker.com` (suffix attack).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EndpointPattern {
    pub host: String,
    #[serde(default = "default_path_prefix")]
    pub path_prefix: String,
    #[serde(default)]
    pub methods: Vec<String>,
}

fn default_path_prefix() -> String {
    "/".to_string()
}

impl EndpointPattern {
    /// Check whether a request (method, host, path) matches this pattern.
    pub fn matches(&self, method: &str, host: &str, path: &str) -> bool {
        // Method check: empty means allow all
        if !self.methods.is_empty()
            && !self.methods.iter().any(|m| m.eq_ignore_ascii_case(method))
        {
            return false;
        }

        // Path prefix check
        if !path.starts_with(&self.path_prefix) {
            return false;
        }

        // Host check
        if self.host.starts_with("*.") {
            // Wildcard: *.domain.com
            // The suffix we need is ".domain.com"
            let suffix = &self.host[1..]; // ".domain.com"
            // host must end with the suffix AND the part before the suffix
            // must not contain a dot (i.e., exactly one subdomain level).
            if !host.ends_with(suffix) {
                return false;
            }
            let prefix = &host[..host.len() - suffix.len()];
            // prefix must be non-empty and must not contain '.'
            !prefix.is_empty() && !prefix.contains('.')
        } else {
            // Exact host match
            host.eq_ignore_ascii_case(&self.host)
        }
    }
}

/// Binds a secret to a specific injection mechanism for matching hosts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CredentialBinding {
    pub secret_name: String,
    pub inject: CredentialInject,
    #[serde(default)]
    pub host_patterns: Vec<String>,
}

/// How a credential is injected into an outbound HTTP request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CredentialInject {
    Bearer,
    Basic { username: String },
    Header { name: String, prefix: Option<String> },
    Query { param_name: String },
    UrlPath { placeholder: String },
}

/// Rate limit for outbound HTTP requests.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimit {
    pub requests_per_minute: u32,
    pub requests_per_hour: u32,
}

/// Grants the ability to invoke other Aleph tools.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolInvokeCapability {
    #[serde(default)]
    pub aliases: HashMap<String, String>,
    #[serde(default = "default_max_per_execution")]
    pub max_per_execution: u32,
}

fn default_max_per_execution() -> u32 {
    20
}

/// Grants access to named secrets matching specific patterns.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretsCapability {
    #[serde(default)]
    pub allowed_patterns: Vec<String>,
}

impl SecretsCapability {
    /// Check whether access to the named secret is allowed.
    ///
    /// Pattern matching rules:
    /// - A pattern ending with `*` matches any name that starts with
    ///   the prefix before the `*` (e.g. `slack_*` matches `slack_token`).
    /// - Otherwise, an exact match is required.
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

#[cfg(test)]
mod tests {
    use super::*;

    // ---------------------------------------------------------------
    // 1. Default capabilities are empty (all None)
    // ---------------------------------------------------------------
    #[test]
    fn test_default_capabilities_are_empty() {
        let caps = WasmCapabilities::default();
        assert!(caps.workspace.is_none());
        assert!(caps.http.is_none());
        assert!(caps.tool_invoke.is_none());
        assert!(caps.secrets.is_none());
    }

    // ---------------------------------------------------------------
    // 2. EndpointPattern matches exact host, correct method and path
    // ---------------------------------------------------------------
    #[test]
    fn test_endpoint_pattern_matches_exact_host() {
        let pattern = EndpointPattern {
            host: "api.example.com".to_string(),
            path_prefix: "/v1/".to_string(),
            methods: vec!["GET".to_string(), "POST".to_string()],
        };

        // Exact match
        assert!(pattern.matches("GET", "api.example.com", "/v1/users"));
        assert!(pattern.matches("POST", "api.example.com", "/v1/data"));

        // Wrong method
        assert!(!pattern.matches("DELETE", "api.example.com", "/v1/users"));

        // Wrong host
        assert!(!pattern.matches("GET", "other.example.com", "/v1/users"));

        // Wrong path prefix
        assert!(!pattern.matches("GET", "api.example.com", "/v2/users"));

        // Empty methods = allow all
        let open_pattern = EndpointPattern {
            host: "api.example.com".to_string(),
            path_prefix: "/".to_string(),
            methods: vec![],
        };
        assert!(open_pattern.matches("DELETE", "api.example.com", "/anything"));
    }

    // ---------------------------------------------------------------
    // 3. Wildcard host matching
    // ---------------------------------------------------------------
    #[test]
    fn test_endpoint_pattern_wildcard_host() {
        let pattern = EndpointPattern {
            host: "*.slack.com".to_string(),
            path_prefix: "/".to_string(),
            methods: vec![],
        };

        // Matches: single subdomain
        assert!(pattern.matches("GET", "api.slack.com", "/"));
        assert!(pattern.matches("GET", "hooks.slack.com", "/webhook"));

        // Does NOT match: bare domain (no subdomain)
        assert!(!pattern.matches("GET", "slack.com", "/"));

        // Does NOT match: suffix attack
        assert!(!pattern.matches("GET", "evil.slack.com.attacker.com", "/"));

        // Does NOT match: nested subdomain (two levels)
        assert!(!pattern.matches("GET", "a.b.slack.com", "/"));
    }

    // ---------------------------------------------------------------
    // 4. All 5 CredentialInject variants can be created
    // ---------------------------------------------------------------
    #[test]
    fn test_credential_inject_variants() {
        let bearer = CredentialInject::Bearer;
        let basic = CredentialInject::Basic {
            username: "user".to_string(),
        };
        let header = CredentialInject::Header {
            name: "X-Api-Key".to_string(),
            prefix: Some("Token ".to_string()),
        };
        let query = CredentialInject::Query {
            param_name: "api_key".to_string(),
        };
        let url_path = CredentialInject::UrlPath {
            placeholder: "{token}".to_string(),
        };

        // Verify they are all distinct Debug representations
        let variants: Vec<String> = vec![
            format!("{:?}", bearer),
            format!("{:?}", basic),
            format!("{:?}", header),
            format!("{:?}", query),
            format!("{:?}", url_path),
        ];
        // All 5 should be unique
        let unique: std::collections::HashSet<_> = variants.iter().collect();
        assert_eq!(unique.len(), 5);

        // Verify serde round-trip for a tagged variant
        let json = serde_json::to_string(&header).unwrap();
        assert!(json.contains("\"type\":\"header\""));
        let deser: CredentialInject = serde_json::from_str(&json).unwrap();
        assert!(matches!(deser, CredentialInject::Header { .. }));
    }

    // ---------------------------------------------------------------
    // 5. SecretsCapability glob and exact pattern matching
    // ---------------------------------------------------------------
    #[test]
    fn test_secrets_capability_pattern_matching() {
        let secrets = SecretsCapability {
            allowed_patterns: vec![
                "slack_*".to_string(),
                "github_token".to_string(),
            ],
        };

        // Glob match
        assert!(secrets.is_allowed("slack_token"));
        assert!(secrets.is_allowed("slack_webhook_url"));
        assert!(secrets.is_allowed("slack_")); // empty suffix still matches prefix

        // Exact match
        assert!(secrets.is_allowed("github_token"));

        // Not allowed
        assert!(!secrets.is_allowed("github_secret"));
        assert!(!secrets.is_allowed("aws_key"));
        assert!(!secrets.is_allowed("slack")); // no underscore, doesn't match "slack_*"
    }
}
