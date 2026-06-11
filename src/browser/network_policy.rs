// SSRF (Server-Side Request Forgery) protection for browser navigation.
// Thin wrapper over the core SSRF engine (`crate::security::ssrf`).

use std::borrow::Cow;
use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::security::ssrf::{self, SsrfPolicy as CoreSsrfPolicy};

/// Configuration for SSRF protection policy.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SsrfConfig {
    /// Block requests to private/loopback networks (default: true).
    #[serde(default = "default_true")]
    pub block_private: bool,

    /// Glob patterns of domains to block (e.g. "*.malware.com", "evil.org").
    #[serde(default)]
    pub blocked_domains: Vec<String>,

    /// If non-empty, only these domains (glob patterns) are allowed (whitelist mode).
    #[serde(default)]
    pub allowed_domains: Vec<String>,

    /// Block navigation to URLs that embed a credential (API key, bearer token,
    /// private key) to prevent secret exfiltration via the URL (default: true).
    /// Orthogonal to SSRF: SSRF guards the destination host, this guards the
    /// URL's content. Enforced only on agent-initiated navigation targets.
    #[serde(default = "default_true")]
    pub block_secrets_in_url: bool,

    /// Redact embedded credentials (API keys, bearer tokens, private keys,
    /// bank/ID numbers) from page-derived text — accessibility snapshots,
    /// console output, network logs, JS-eval results — before it is returned to
    /// the LLM (default: true). Symmetric to [`Self::block_secrets_in_url`]:
    /// that guards the navigation *target* (secrets going OUT via the URL);
    /// this guards page-content *egress* (secrets coming back from the page into
    /// the model context, long-term memory, and provider requests). Set to
    /// `false` for trusted self-hosted pages where the agent legitimately needs
    /// to read credential-shaped values verbatim.
    #[serde(default = "default_true")]
    pub redact_secrets_in_content: bool,
}

const fn default_true() -> bool {
    true
}

impl Default for SsrfConfig {
    fn default() -> Self {
        Self {
            block_private: true,
            blocked_domains: Vec::new(),
            allowed_domains: Vec::new(),
            block_secrets_in_url: true,
            redact_secrets_in_content: true,
        }
    }
}

/// Reasons a URL can be rejected by SSRF policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyViolation {
    /// The host resolves to a private/loopback network address.
    PrivateNetwork(String),
    /// The domain matches a blocked pattern.
    BlockedDomain(String),
    /// The domain is not in the allowed whitelist.
    NotInAllowlist(String),
    /// The URL could not be parsed.
    InvalidUrl(String),
    /// The URL embeds a credential (potential secret exfiltration). Carries the
    /// matched secret-rule name (e.g. "api_key").
    SecretInUrl(String),
}

impl fmt::Display for PolicyViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PrivateNetwork(host) => {
                write!(f, "blocked: host '{host}' resolves to a private network")
            }
            Self::BlockedDomain(domain) => {
                write!(f, "blocked: domain '{domain}' matches a block pattern")
            }
            Self::NotInAllowlist(domain) => {
                write!(f, "blocked: domain '{domain}' is not in the allowlist")
            }
            Self::InvalidUrl(reason) => {
                write!(f, "invalid URL: {reason}")
            }
            Self::SecretInUrl(rule) => {
                write!(
                    f,
                    "blocked: URL embeds a secret ({rule}) — refusing to exfiltrate a credential via navigation"
                )
            }
        }
    }
}

impl std::error::Error for PolicyViolation {}

impl From<ssrf::SsrfError> for PolicyViolation {
    fn from(err: ssrf::SsrfError) -> Self {
        match &err {
            ssrf::SsrfError::InvalidUrl(_) | ssrf::SsrfError::NoHost => {
                Self::InvalidUrl(err.to_string())
            }
            ssrf::SsrfError::BlockedAddress(addr) => {
                // Distinguish private-network blocks from domain blocks.
                // The core engine prefixes blocklist hits with "host in blocklist: ".
                if let Some(domain) = addr.strip_prefix("host in blocklist: ") {
                    Self::BlockedDomain(domain.to_string())
                } else {
                    Self::PrivateNetwork(addr.clone())
                }
            }
            ssrf::SsrfError::DnsResolutionFailed { host, .. } => {
                Self::InvalidUrl(format!("DNS resolution failed for host: {host}"))
            }
            ssrf::SsrfError::TooManyRedirects(_) | ssrf::SsrfError::FetchFailed(_) => {
                Self::InvalidUrl(err.to_string())
            }
        }
    }
}

/// SSRF protection guard for browser navigation.
/// Delegates to the core SSRF engine for IP/hostname validation.
#[derive(Debug, Clone, Default)]
pub struct BrowserSsrfGuard {
    config: SsrfConfig,
}

impl BrowserSsrfGuard {
    #[must_use]
    pub const fn new(config: SsrfConfig) -> Self {
        Self { config }
    }

    /// Validate a URL against the SSRF policy.
    pub fn check_url(&self, url_str: &str) -> Result<(), PolicyViolation> {
        // Browser navigation is web-only: reject any non-HTTP(S) scheme up-front.
        // The sync SSRF engine validates the *host* but not the scheme, so a
        // host-bearing alternate scheme (e.g. `gopher://internal:6379/…`,
        // `ftp://host/…`) would otherwise pass the host checks — a classic
        // SSRF-via-alternate-scheme vector. Schemeless / hostless URLs
        // (`about:blank`, `data:`, `file:///…`) still fall through to the
        // engine's no-host rejection below.
        if let Ok(parsed) = url::Url::parse(url_str) {
            let scheme = parsed.scheme();
            if scheme != "http" && scheme != "https" {
                return Err(PolicyViolation::InvalidUrl(format!(
                    "unsupported scheme '{scheme}': browser navigation allows only http/https"
                )));
            }
        }

        // Build core policy from browser config.
        // When block_private is false, disable SSRF protection entirely so that
        // loopback, localhost, and private ranges are all reachable (useful for
        // local development and self-hosted deployments).
        let core_policy = if !self.config.block_private
            && self.config.blocked_domains.is_empty()
            && self.config.allowed_domains.is_empty()
        {
            CoreSsrfPolicy::disabled()
        } else {
            CoreSsrfPolicy {
                enabled: true,
                allow_private_network: !self.config.block_private,
                allowed_hosts: self.config.allowed_domains.clone(),
                blocked_hosts: self.config.blocked_domains.clone(),
                ..CoreSsrfPolicy::default()
            }
        };

        // Delegate to core engine (sync validation)
        ssrf::validate_url(url_str, &core_policy).map_err(PolicyViolation::from)?;

        // Additional browser-specific: allowlist-only mode
        // (when allowed_domains is non-empty, ONLY those domains are permitted)
        if !self.config.allowed_domains.is_empty() {
            let url =
                url::Url::parse(url_str).map_err(|e| PolicyViolation::InvalidUrl(e.to_string()))?;
            if let Some(host) = url.host_str() {
                // The core engine's allowlist BYPASSES blocks, but browser needs allowlist-ONLY mode.
                // Check if host matches any allowed domain pattern.
                let host_lower = host.to_ascii_lowercase();
                let matched = self.config.allowed_domains.iter().any(|pat| {
                    let pat_lower = pat.to_ascii_lowercase();
                    if let Some(base) = pat_lower.strip_prefix("*.") {
                        host_lower == base || host_lower.ends_with(&format!(".{base}"))
                    } else {
                        host_lower == pat_lower
                    }
                });
                if !matched {
                    return Err(PolicyViolation::NotInAllowlist(host.to_string()));
                }
            }
        }

        Ok(())
    }

    /// Validate a URL for an **agent-initiated navigation target**: the full
    /// SSRF policy ([`Self::check_url`]) plus, when `block_secrets_in_url` is
    /// set, a scan for embedded credentials. Use this for `goto`/`open`; the
    /// post-navigation active-URL re-check stays on [`Self::check_url`] so a
    /// landed page whose URL legitimately carries a token is still readable.
    pub fn check_navigation(&self, url_str: &str) -> Result<(), PolicyViolation> {
        self.check_url(url_str)?;
        if self.config.block_secrets_in_url {
            if let Some(rule) = super::secret_guard::scan_url_for_secrets(url_str) {
                return Err(PolicyViolation::SecretInUrl(rule));
            }
        }
        Ok(())
    }

    /// Redact embedded credentials from page-derived `text` before it is handed
    /// back to the LLM, when `redact_secrets_in_content` is set. This is the OUT
    /// half of the secret-egress boundary (page content → model); the navigation
    /// guards above are the IN half (model context → navigation URL). Returns
    /// the input unchanged (zero-copy) when redaction is disabled or no secret
    /// is present.
    #[must_use]
    pub fn redact_content<'a>(&self, text: &'a str) -> Cow<'a, str> {
        if self.config.redact_secrets_in_content {
            super::secret_guard::redact_secrets(text)
        } else {
            Cow::Borrowed(text)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_blocks_localhost() {
        let policy = BrowserSsrfGuard::default();

        assert!(matches!(
            policy.check_url("http://localhost/path"),
            Err(PolicyViolation::PrivateNetwork(_))
        ));
        assert!(matches!(
            policy.check_url("http://127.0.0.1:8080/api"),
            Err(PolicyViolation::PrivateNetwork(_))
        ));
        assert!(matches!(
            policy.check_url("http://[::1]/"),
            Err(PolicyViolation::PrivateNetwork(_))
        ));
    }

    #[test]
    fn test_blocks_private_networks() {
        let policy = BrowserSsrfGuard::default();

        // 10.x.x.x
        assert!(matches!(
            policy.check_url("http://10.0.0.1/"),
            Err(PolicyViolation::PrivateNetwork(_))
        ));
        // 172.16.x.x
        assert!(matches!(
            policy.check_url("http://172.16.0.1/"),
            Err(PolicyViolation::PrivateNetwork(_))
        ));
        // 172.31.x.x (upper bound)
        assert!(matches!(
            policy.check_url("http://172.31.255.255/"),
            Err(PolicyViolation::PrivateNetwork(_))
        ));
        // 192.168.x.x
        assert!(matches!(
            policy.check_url("http://192.168.1.1/"),
            Err(PolicyViolation::PrivateNetwork(_))
        ));
    }

    #[test]
    fn test_allows_public_urls() {
        let policy = BrowserSsrfGuard::default();

        assert!(policy.check_url("https://example.com/page").is_ok());
        assert!(policy.check_url("https://8.8.8.8/dns").is_ok());
        assert!(policy.check_url("https://172.32.0.1/").is_ok()); // 172.32 is NOT private
    }

    #[test]
    fn test_blocked_domain_patterns() {
        let policy = BrowserSsrfGuard::new(SsrfConfig {
            block_private: false,
            blocked_domains: vec!["*.malware.com".to_string(), "evil.org".to_string()],
            allowed_domains: vec![],
            block_secrets_in_url: false,
            redact_secrets_in_content: false,
        });

        // Subdomain match
        assert!(
            policy.check_url("https://payload.malware.com/x").is_err(),
            "subdomain of blocked wildcard should be blocked"
        );
        // Bare domain match for wildcard
        assert!(
            policy.check_url("https://malware.com/x").is_err(),
            "bare domain of blocked wildcard should be blocked"
        );
        // Exact match
        assert!(
            policy.check_url("https://evil.org/").is_err(),
            "exact blocked domain should be blocked"
        );
        // Non-matching domain is fine
        assert!(policy.check_url("https://safe.com/").is_ok());
    }

    #[test]
    fn test_allowed_domains_whitelist() {
        let policy = BrowserSsrfGuard::new(SsrfConfig {
            block_private: false,
            blocked_domains: vec![],
            allowed_domains: vec!["*.trusted.com".to_string(), "api.example.org".to_string()],
            block_secrets_in_url: false,
            redact_secrets_in_content: false,
        });

        // Allowed
        assert!(policy.check_url("https://app.trusted.com/").is_ok());
        assert!(policy.check_url("https://api.example.org/v1").is_ok());

        // Not in allowlist
        assert!(matches!(
            policy.check_url("https://random.com/"),
            Err(PolicyViolation::NotInAllowlist(_))
        ));
    }

    #[test]
    fn test_disabled_ssrf_allows_everything() {
        let policy = BrowserSsrfGuard::new(SsrfConfig {
            block_private: false,
            blocked_domains: vec![],
            allowed_domains: vec![],
            block_secrets_in_url: false,
            redact_secrets_in_content: false,
        });

        assert!(policy.check_url("http://localhost/").is_ok());
        assert!(policy.check_url("http://10.0.0.1/").is_ok());
        assert!(policy.check_url("http://192.168.1.1/").is_ok());
        assert!(policy.check_url("https://example.com/").is_ok());
    }

    #[test]
    fn test_invalid_url() {
        let policy = BrowserSsrfGuard::default();

        assert!(matches!(
            policy.check_url("not-a-url"),
            Err(PolicyViolation::InvalidUrl(_))
        ));
    }

    #[test]
    fn test_rejects_non_http_schemes() {
        // Host-bearing alternate schemes must not slip past the host-only SSRF
        // checks (SSRF-via-alternate-scheme). Disable host blocking so the only
        // thing that can reject these is the scheme guard itself.
        let policy = BrowserSsrfGuard::new(SsrfConfig {
            block_private: false,
            blocked_domains: vec![],
            allowed_domains: vec![],
            block_secrets_in_url: false,
            redact_secrets_in_content: false,
        });
        for url in [
            "gopher://internal-host:6379/_data",
            "ftp://files.example.com/etc/passwd",
            "file:///etc/passwd",
            "data:text/html,<script>alert(1)</script>",
        ] {
            assert!(
                matches!(policy.check_url(url), Err(PolicyViolation::InvalidUrl(_))),
                "scheme of {url} should be rejected"
            );
        }
        // http/https still pass when host policy is disabled.
        assert!(policy.check_url("http://example.com/").is_ok());
        assert!(policy.check_url("https://example.com/").is_ok());
    }

    #[test]
    fn test_policy_violation_display() {
        let v = PolicyViolation::PrivateNetwork("127.0.0.1".to_string());
        assert!(v.to_string().contains("private network"));

        let v = PolicyViolation::BlockedDomain("evil.com".to_string());
        assert!(v.to_string().contains("block pattern"));

        let v = PolicyViolation::NotInAllowlist("random.com".to_string());
        assert!(v.to_string().contains("allowlist"));

        let v = PolicyViolation::InvalidUrl("missing scheme".to_string());
        assert!(v.to_string().contains("invalid URL"));

        let v = PolicyViolation::SecretInUrl("api_key".to_string());
        assert!(v.to_string().contains("secret"));
    }

    #[test]
    fn check_navigation_blocks_secret_in_url() {
        // Default guard has block_secrets_in_url = true.
        let policy = BrowserSsrfGuard::default();
        let url = "https://public.example/?leak=sk-ant-api03-0123456789abcdefghijklmnop";
        // SSRF alone allows the public host…
        assert!(policy.check_url(url).is_ok());
        // …but navigation rejects the embedded credential.
        assert!(matches!(
            policy.check_navigation(url),
            Err(PolicyViolation::SecretInUrl(_))
        ));
        // Clean public URL still navigates.
        assert!(policy
            .check_navigation("https://public.example/docs")
            .is_ok());
    }

    #[test]
    fn check_navigation_respects_disabled_secret_scan() {
        let policy = BrowserSsrfGuard::new(SsrfConfig {
            block_private: false,
            blocked_domains: vec![],
            allowed_domains: vec![],
            block_secrets_in_url: false,
            redact_secrets_in_content: false,
        });
        let url = "https://public.example/?leak=sk-ant-api03-0123456789abcdefghijklmnop";
        assert!(policy.check_navigation(url).is_ok());
    }

    #[test]
    fn redact_content_scrubs_secrets_by_default() {
        // Default guard has redact_secrets_in_content = true.
        let policy = BrowserSsrfGuard::default();
        let page = "API token: sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789 shown";
        let out = policy.redact_content(page);
        assert!(!out.contains("sk-ant-api03"));
        assert!(out.contains("[REDACTED:"));
        // Clean page content is returned untouched (zero-copy).
        let clean = "- button \"Submit\" [ref=e1]";
        assert!(matches!(policy.redact_content(clean), Cow::Borrowed(_)));
    }

    #[test]
    fn redact_content_noop_when_disabled() {
        let policy = BrowserSsrfGuard::new(SsrfConfig {
            block_private: true,
            blocked_domains: vec![],
            allowed_domains: vec![],
            block_secrets_in_url: true,
            redact_secrets_in_content: false,
        });
        let page = "API token: sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789 shown";
        let out = policy.redact_content(page);
        assert_eq!(out, page);
        assert!(matches!(out, Cow::Borrowed(_)));
    }
}
