//! Credential injection at the host boundary.
//!
//! Resolves credentials and injects them into outbound HTTP requests so that
//! WASM plugins never see secret values. The host looks up the secret by
//! name, matches the target URL against declared host patterns, and applies
//! the appropriate injection strategy (Bearer, Basic, Header, Query, UrlPath).

use url::Url;

use super::capabilities::{host_matches_pattern, CredentialBinding, CredentialInject};

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors that can occur during credential injection.
#[derive(Debug)]
pub enum CredentialError {
    /// The named secret was not found in the provided secrets slice.
    SecretNotFound(String),
    /// The URL could not be parsed.
    InvalidUrl(String),
}

impl std::fmt::Display for CredentialError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CredentialError::SecretNotFound(name) => {
                write!(f, "secret not found: {}", name)
            }
            CredentialError::InvalidUrl(reason) => {
                write!(f, "invalid URL: {}", reason)
            }
        }
    }
}

impl std::error::Error for CredentialError {}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Inject a credential into an HTTP request if the URL host matches.
///
/// # Returns
///
/// - `Ok(Some(modified_url))` if the URL was modified (`Query` / `UrlPath`).
/// - `Ok(None)` if only headers were modified (`Bearer` / `Basic` / `Header`),
///   or if the URL host did not match any `binding.host_patterns` (silent skip).
/// - `Err(CredentialError::SecretNotFound)` if the host matched but the named
///   secret was not present in `secrets`.
/// - `Err(CredentialError::InvalidUrl)` if the URL could not be parsed.
pub fn inject_credential(
    binding: &CredentialBinding,
    url: &str,
    headers: &mut Vec<(String, String)>,
    secrets: &[(String, String)],
) -> Result<Option<String>, CredentialError> {
    // 1. Parse URL to extract host.
    let parsed =
        Url::parse(url).map_err(|e| CredentialError::InvalidUrl(format!("{}", e)))?;

    let host = parsed
        .host_str()
        .ok_or_else(|| CredentialError::InvalidUrl("no host in URL".to_string()))?;

    // 2. Check if URL host matches any binding host pattern.
    let host_matched = binding
        .host_patterns
        .iter()
        .any(|pat| host_matches_pattern(host, pat));

    if !host_matched {
        // Host does not match any pattern -- skip silently.
        return Ok(None);
    }

    // 3. Look up the secret value.
    let secret_value = secrets
        .iter()
        .find(|(name, _)| name == &binding.secret_name)
        .map(|(_, value)| value.as_str())
        .ok_or_else(|| CredentialError::SecretNotFound(binding.secret_name.clone()))?;

    // 4. Inject based on the variant.
    match &binding.inject {
        CredentialInject::Bearer => {
            headers.push((
                "Authorization".to_string(),
                format!("Bearer {}", secret_value),
            ));
            Ok(None)
        }

        CredentialInject::Basic { username } => {
            use base64::{engine::general_purpose, Engine as _};
            let encoded =
                general_purpose::STANDARD.encode(format!("{}:{}", username, secret_value));
            headers.push((
                "Authorization".to_string(),
                format!("Basic {}", encoded),
            ));
            Ok(None)
        }

        CredentialInject::Header { name, prefix } => {
            let value = match prefix {
                Some(p) => format!("{}{}", p, secret_value),
                None => secret_value.to_string(),
            };
            headers.push((name.clone(), value));
            Ok(None)
        }

        CredentialInject::Query { param_name } => {
            let mut parsed = parsed;
            parsed
                .query_pairs_mut()
                .append_pair(param_name, secret_value);
            Ok(Some(parsed.to_string()))
        }

        CredentialInject::UrlPath { placeholder } => {
            let modified = url.replace(placeholder, secret_value);
            Ok(Some(modified))
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::runtime::wasm::capabilities::{CredentialBinding, CredentialInject};

    // ---------------------------------------------------------------
    // 1. Bearer token injection
    // ---------------------------------------------------------------
    #[test]
    fn test_bearer_injection() {
        let binding = CredentialBinding {
            secret_name: "slack_token".to_string(),
            inject: CredentialInject::Bearer,
            host_patterns: vec!["slack.com".to_string()],
        };

        let mut headers = Vec::new();
        let secrets = vec![("slack_token".to_string(), "xoxb-secret-value".to_string())];

        let result =
            inject_credential(&binding, "https://slack.com/api/test", &mut headers, &secrets);

        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
        assert_eq!(headers.len(), 1);
        assert_eq!(headers[0].0, "Authorization");
        assert_eq!(headers[0].1, "Bearer xoxb-secret-value");
    }

    // ---------------------------------------------------------------
    // 2. Header injection with prefix
    // ---------------------------------------------------------------
    #[test]
    fn test_header_injection_with_prefix() {
        let binding = CredentialBinding {
            secret_name: "api_key".to_string(),
            inject: CredentialInject::Header {
                name: "X-API-Key".to_string(),
                prefix: Some("Key ".to_string()),
            },
            host_patterns: vec!["api.example.com".to_string()],
        };

        let mut headers = Vec::new();
        let secrets = vec![("api_key".to_string(), "my-secret-key".to_string())];

        let result = inject_credential(
            &binding,
            "https://api.example.com/v1/data",
            &mut headers,
            &secrets,
        );

        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
        assert_eq!(headers.len(), 1);
        assert_eq!(headers[0].0, "X-API-Key");
        assert_eq!(headers[0].1, "Key my-secret-key");
    }

    // ---------------------------------------------------------------
    // 3. Query parameter injection
    // ---------------------------------------------------------------
    #[test]
    fn test_query_injection() {
        let binding = CredentialBinding {
            secret_name: "google_key".to_string(),
            inject: CredentialInject::Query {
                param_name: "key".to_string(),
            },
            host_patterns: vec!["maps.googleapis.com".to_string()],
        };

        let mut headers = Vec::new();
        let secrets = vec![("google_key".to_string(), "AIza-test-key".to_string())];

        let result = inject_credential(
            &binding,
            "https://maps.googleapis.com/api/geocode",
            &mut headers,
            &secrets,
        );

        assert!(result.is_ok());
        let modified_url = result.unwrap();
        assert!(modified_url.is_some());
        let url = modified_url.unwrap();
        assert!(
            url.contains("key=AIza-test-key"),
            "Expected URL to contain 'key=AIza-test-key', got: {}",
            url
        );
        // Headers should be untouched.
        assert!(headers.is_empty());
    }

    // ---------------------------------------------------------------
    // 4. Host pattern mismatch skips silently
    // ---------------------------------------------------------------
    #[test]
    fn test_host_pattern_mismatch_skips() {
        let binding = CredentialBinding {
            secret_name: "slack_token".to_string(),
            inject: CredentialInject::Bearer,
            host_patterns: vec!["slack.com".to_string()],
        };

        let mut headers = Vec::new();
        let secrets = vec![("slack_token".to_string(), "xoxb-secret".to_string())];

        let result =
            inject_credential(&binding, "https://evil.com/api/steal", &mut headers, &secrets);

        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
        // No headers should have been added.
        assert!(headers.is_empty());
    }

    // ---------------------------------------------------------------
    // 5. Missing secret with matching host returns error
    // ---------------------------------------------------------------
    #[test]
    fn test_missing_secret_errors() {
        let binding = CredentialBinding {
            secret_name: "nonexistent_secret".to_string(),
            inject: CredentialInject::Bearer,
            host_patterns: vec!["slack.com".to_string()],
        };

        let mut headers = Vec::new();
        let secrets: Vec<(String, String)> = vec![]; // empty

        let result =
            inject_credential(&binding, "https://slack.com/api/test", &mut headers, &secrets);

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, CredentialError::SecretNotFound(ref name) if name == "nonexistent_secret"),
            "Expected SecretNotFound, got: {:?}",
            err
        );
    }

    // ---------------------------------------------------------------
    // Bonus: Basic auth injection
    // ---------------------------------------------------------------
    #[test]
    fn test_basic_auth_injection() {
        let binding = CredentialBinding {
            secret_name: "jira_password".to_string(),
            inject: CredentialInject::Basic {
                username: "admin".to_string(),
            },
            host_patterns: vec!["jira.example.com".to_string()],
        };

        let mut headers = Vec::new();
        let secrets = vec![("jira_password".to_string(), "s3cret".to_string())];

        let result = inject_credential(
            &binding,
            "https://jira.example.com/rest/api/2/issue",
            &mut headers,
            &secrets,
        );

        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
        assert_eq!(headers.len(), 1);
        assert_eq!(headers[0].0, "Authorization");

        // Verify Base64 encoding: "admin:s3cret" -> "YWRtaW46czNjcmV0"
        use base64::{engine::general_purpose, Engine as _};
        let expected =
            format!("Basic {}", general_purpose::STANDARD.encode("admin:s3cret"));
        assert_eq!(headers[0].1, expected);
    }

    // ---------------------------------------------------------------
    // Bonus: UrlPath placeholder replacement
    // ---------------------------------------------------------------
    #[test]
    fn test_url_path_injection() {
        let binding = CredentialBinding {
            secret_name: "webhook_token".to_string(),
            inject: CredentialInject::UrlPath {
                placeholder: "{token}".to_string(),
            },
            host_patterns: vec!["hooks.slack.com".to_string()],
        };

        let mut headers = Vec::new();
        let secrets = vec![("webhook_token".to_string(), "T0K3N-VALUE".to_string())];

        let result = inject_credential(
            &binding,
            "https://hooks.slack.com/services/{token}/B123",
            &mut headers,
            &secrets,
        );

        assert!(result.is_ok());
        let modified_url = result.unwrap();
        assert!(modified_url.is_some());
        let url = modified_url.unwrap();
        assert_eq!(url, "https://hooks.slack.com/services/T0K3N-VALUE/B123");
        assert!(headers.is_empty());
    }

    // ---------------------------------------------------------------
    // Bonus: Wildcard host pattern matching
    // ---------------------------------------------------------------
    #[test]
    fn test_wildcard_host_pattern() {
        let binding = CredentialBinding {
            secret_name: "slack_token".to_string(),
            inject: CredentialInject::Bearer,
            host_patterns: vec!["*.slack.com".to_string()],
        };

        let mut headers = Vec::new();
        let secrets = vec![("slack_token".to_string(), "xoxb-123".to_string())];

        // Should match: single subdomain level
        let result = inject_credential(
            &binding,
            "https://api.slack.com/methods",
            &mut headers,
            &secrets,
        );
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
        assert_eq!(headers.len(), 1);
        assert_eq!(headers[0].1, "Bearer xoxb-123");

        // Should NOT match: bare domain
        headers.clear();
        let result = inject_credential(
            &binding,
            "https://slack.com/api/test",
            &mut headers,
            &secrets,
        );
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
        assert!(headers.is_empty(), "bare domain should not match wildcard");
    }

    // ---------------------------------------------------------------
    // Unit tests for host_matches_pattern
    // ---------------------------------------------------------------
    #[test]
    fn test_host_matches_pattern_exact() {
        assert!(host_matches_pattern("slack.com", "slack.com"));
        assert!(host_matches_pattern("Slack.COM", "slack.com"));
        assert!(!host_matches_pattern("evil.com", "slack.com"));
    }

    #[test]
    fn test_host_matches_pattern_wildcard() {
        assert!(host_matches_pattern("api.slack.com", "*.slack.com"));
        assert!(host_matches_pattern("hooks.slack.com", "*.slack.com"));
        // No match: bare domain
        assert!(!host_matches_pattern("slack.com", "*.slack.com"));
        // No match: nested subdomain
        assert!(!host_matches_pattern("a.b.slack.com", "*.slack.com"));
    }
}
