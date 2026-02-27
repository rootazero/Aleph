//! HTTP endpoint allowlist validator with anti-bypass security measures.
//!
//! Validates outbound HTTP requests from WASM plugins against a declared
//! allowlist of endpoint patterns. Enforces HTTPS-only, rejects userinfo
//! in URLs, and detects path traversal attempts (including percent-encoded).

use std::fmt;

use percent_encoding::percent_decode_str;
use url::Url;

use super::capabilities::EndpointPattern;

/// Errors returned when an HTTP request fails allowlist validation.
#[derive(Debug)]
pub enum AllowlistError {
    /// The URL scheme is not HTTPS.
    HttpsRequired,
    /// The URL is malformed or contains suspicious components (e.g. userinfo).
    InvalidUrl(String),
    /// The URL path contains traversal sequences (`..`).
    PathTraversal,
    /// The request does not match any allowlisted endpoint pattern.
    NotAllowed(String),
}

impl fmt::Display for AllowlistError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AllowlistError::HttpsRequired => write!(f, "HTTPS is required; HTTP is not allowed"),
            AllowlistError::InvalidUrl(reason) => write!(f, "Invalid URL: {}", reason),
            AllowlistError::PathTraversal => {
                write!(f, "Path traversal detected: URL path contains '..'")
            }
            AllowlistError::NotAllowed(detail) => {
                write!(f, "Request not allowed by allowlist: {}", detail)
            }
        }
    }
}

impl std::error::Error for AllowlistError {}

/// Validates HTTP requests against a set of allowed endpoint patterns.
///
/// Performs layered security checks before matching against the allowlist:
/// 1. HTTPS enforcement
/// 2. Userinfo rejection (anti host-confusion)
/// 3. Path traversal detection (raw and percent-encoded)
pub struct AllowlistValidator {
    patterns: Vec<EndpointPattern>,
}

impl AllowlistValidator {
    /// Create a new validator with the given endpoint patterns.
    pub fn new(patterns: Vec<EndpointPattern>) -> Self {
        Self { patterns }
    }

    /// Validate an HTTP request against the allowlist.
    ///
    /// # Arguments
    /// * `method` - HTTP method (e.g. "GET", "POST")
    /// * `url_str` - Full URL string to validate
    ///
    /// # Security checks (in order)
    /// 1. Parse the URL
    /// 2. Reject non-HTTPS schemes
    /// 3. Reject URLs with userinfo (`@` in authority)
    /// 4. Extract host
    /// 5. Reject path traversal (`..` in raw path)
    /// 6. Reject percent-encoded path traversal
    /// 7. Match against allowlist patterns
    pub fn check(&self, method: &str, url_str: &str) -> Result<(), AllowlistError> {
        // 1. Parse URL
        let parsed = Url::parse(url_str)
            .map_err(|e| AllowlistError::InvalidUrl(format!("failed to parse: {}", e)))?;

        // 2. HTTPS only
        if parsed.scheme() != "https" {
            return Err(AllowlistError::HttpsRequired);
        }

        // 3. Reject userinfo (anti host-confusion: https://victim@attacker.com)
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err(AllowlistError::InvalidUrl(
                "URL must not contain userinfo (username/password)".to_string(),
            ));
        }

        // 4. Extract host
        let host = parsed
            .host_str()
            .ok_or_else(|| AllowlistError::InvalidUrl("no host in URL".to_string()))?;

        // 5. Path traversal check on the RAW URL string.
        //    We must check the original string because url::Url resolves `..`
        //    segments during parsing per the WHATWG URL Standard, which would
        //    silently normalize away traversal attempts.
        let raw_path = extract_raw_path(url_str);
        if path_contains_traversal(raw_path) {
            return Err(AllowlistError::PathTraversal);
        }

        // 6. Percent-encoded traversal check on the raw path
        let decoded = percent_decode_str(raw_path).decode_utf8_lossy();
        if path_contains_traversal(&decoded) {
            return Err(AllowlistError::PathTraversal);
        }

        // 7. Match against allowlist using the normalized (parsed) path
        let path = parsed.path();
        let matched = self
            .patterns
            .iter()
            .any(|p| p.matches(method, host, path));

        if matched {
            Ok(())
        } else {
            Err(AllowlistError::NotAllowed(format!(
                "{} {} does not match any allowed endpoint",
                method, url_str
            )))
        }
    }
}

/// Extract the path component from a raw URL string without normalization.
///
/// The `url::Url` parser resolves `..` segments per the WHATWG URL Standard,
/// which would hide traversal attempts. This function extracts the raw path
/// so we can detect `..` before normalization erases it.
///
/// For `https://host:port/path?query#frag`, returns `/path`.
fn extract_raw_path(url_str: &str) -> &str {
    // Skip past the scheme ("https://")
    let after_scheme = url_str
        .find("://")
        .map(|i| &url_str[i + 3..])
        .unwrap_or(url_str);

    // Skip past the authority (host[:port]) — find the first '/'
    let path_start = after_scheme.find('/').unwrap_or(after_scheme.len());
    let path_and_rest = &after_scheme[path_start..];

    // Trim query string and fragment
    let path = path_and_rest
        .split_once('?')
        .map(|(p, _)| p)
        .unwrap_or(path_and_rest);
    let path = path
        .split_once('#')
        .map(|(p, _)| p)
        .unwrap_or(path);

    path
}

/// Check whether a path string contains traversal sequences.
///
/// Splits by `/` and checks each segment for `..` to avoid false positives
/// on legitimate path components that happen to contain the substring `..`.
fn path_contains_traversal(path: &str) -> bool {
    path.split('/').any(|segment| segment == "..")
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let err = v
            .check("GET", "http://slack.com/api/users.list")
            .unwrap_err();
        assert!(matches!(err, AllowlistError::HttpsRequired));
    }

    #[test]
    fn test_rejects_userinfo() {
        let v = slack_allowlist();
        let err = v
            .check("GET", "https://slack.com@evil.com/api/steal")
            .unwrap_err();
        assert!(
            matches!(err, AllowlistError::InvalidUrl(_) | AllowlistError::NotAllowed(_)),
            "Expected InvalidUrl or NotAllowed, got: {:?}",
            err
        );
    }

    #[test]
    fn test_rejects_unlisted_host() {
        let v = slack_allowlist();
        let err = v
            .check("GET", "https://evil.com/api/users.list")
            .unwrap_err();
        assert!(matches!(err, AllowlistError::NotAllowed(_)));
    }

    #[test]
    fn test_rejects_unlisted_method() {
        let v = slack_allowlist();
        let err = v
            .check("DELETE", "https://slack.com/api/users.list")
            .unwrap_err();
        assert!(matches!(err, AllowlistError::NotAllowed(_)));
    }

    #[test]
    fn test_rejects_path_traversal() {
        let v = slack_allowlist();
        let err = v
            .check("GET", "https://slack.com/api/../etc/passwd")
            .unwrap_err();
        assert!(matches!(err, AllowlistError::PathTraversal));
    }
}
