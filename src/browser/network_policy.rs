// SSRF (Server-Side Request Forgery) protection for browser navigation.
// Thin wrapper over the core SSRF engine (`crate::security::ssrf`).

use std::borrow::Cow;
use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::security::ssrf::ip::is_ip_blocked_by_policy;
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

    /// Block form input (type/fill/select/dialog prompt text) that embeds a
    /// credential to prevent secret exfiltration via a form field on an
    /// otherwise policy-allowed host (default: true). Guards secrets going OUT
    /// via form input — symmetric to [`Self::block_secrets_in_url`], which
    /// guards secrets going OUT via the navigation URL.
    #[serde(default = "default_true")]
    pub block_secrets_in_input: bool,

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
            block_secrets_in_input: true,
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
    /// matched secret-rule name (e.g. "`api_key`").
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

    /// Build the core SSRF policy from browser config.
    /// When `block_private` is false and no allow/blocklists are configured,
    /// the policy is disabled entirely so that loopback, localhost, and
    /// private ranges are all reachable (useful for local development and
    /// self-hosted deployments).
    fn build_core_policy(&self) -> CoreSsrfPolicy {
        if !self.config.block_private
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
        }
    }

    /// Validate a URL against the SSRF policy.
    pub async fn check_url(&self, url_str: &str) -> Result<(), PolicyViolation> {
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

        let core_policy = self.build_core_policy();

        // Delegate to core engine (async validation) — resolves hostnames and
        // validates every returned IP against the blocklist, so a hostname
        // that currently maps to loopback / private / link-local / metadata
        // is rejected before being handed to Playwright/Chrome.
        ssrf::validate_url_async(url_str, &core_policy)
            .await
            .map(|(_, _pinned)| ())
            .map_err(PolicyViolation::from)?;

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

    /// Build a Chrome `--host-resolver-rules` MAP argument that pins the
    /// URL's hostname to the IPs returned by a fresh DNS lookup, after each
    /// IP has been re-validated via `is_ip_blocked_by_policy`.
    ///
    /// This is the second DNS-validation step that closes the rebinding
    /// window between [`Self::check_url`] (which resolves once) and Chrome's
    /// own resolver (which may resolve again, possibly to a different set of
    /// IPs). Callers must invoke [`Self::check_url`] (or
    /// [`Self::check_navigation`]) first; this method does **not** re-run
    /// domain blocklist / allowlist / secret-scan checks.
    ///
    /// Returns:
    /// - `Ok(None)` for IP-literal URLs (Chrome resolves the address directly),
    ///   schemeless/hostless URLs, and when the policy is disabled (no IP
    ///   validation possible).
    /// - `Ok(Some(arg))` for hostnames whose returned IPs include at least one
    ///   IP that passes the policy — the arg lists every passing IP comma-
    ///   separated so Chrome can round-robin them.
    /// - `Err(PolicyViolation)` when DNS resolution fails or every returned IP
    ///   is blocked by the policy (the classic rebinding-into-loopback case).
    ///
    /// Residual TOCTOU: this method and `check_url` observe the same DNS
    /// snapshot within a single async task, so a rebinding race between the
    /// two lookups requires an attacker to mutate the OS resolver from
    /// outside the process — outside what this layer can deterministically
    /// test. The defense-in-depth contract: every IP Chrome is allowed to use
    /// has been validated at least once via `is_ip_blocked_by_policy` before
    /// the process starts.
    pub async fn pin_host_resolver_args(
        &self,
        url_str: &str,
    ) -> Result<Option<String>, PolicyViolation> {
        let url =
            url::Url::parse(url_str).map_err(|e| PolicyViolation::InvalidUrl(e.to_string()))?;

        // Scheme floor (same policy as check_url): non-http/https schemes must
        // not produce a MAP rule even if the host would pass.
        let scheme = url.scheme();
        if scheme != "http" && scheme != "https" {
            return Err(PolicyViolation::InvalidUrl(format!(
                "unsupported scheme '{scheme}': browser navigation allows only http/https"
            )));
        }

        let host = match url.host_str() {
            Some(h) => h,
            None => return Ok(None),
        };

        // IP literal → no hostname to MAP.
        if matches!(url.host(), Some(url::Host::Ipv4(_) | url::Host::Ipv6(_))) {
            return Ok(None);
        }

        let core_policy = self.build_core_policy();

        // Disabled SSRF → no IP validation possible, skip pinning rather than
        // hand Chrome an unvalidated MAP.
        if !core_policy.enabled {
            return Ok(None);
        }

        // Re-resolve and filter (rebinding defense). All returned IPs are
        // classified; only those that pass `is_ip_blocked_by_policy` are
        // included in the MAP. If none pass, surface as PrivateNetwork so the
        // caller sees the same violation shape as `check_url`.
        let port = url.port_or_known_default().unwrap_or(80);
        let all_ips = ssrf::dns::lookup_all(host, port)
            .await
            .map_err(PolicyViolation::from)?;

        let passing: Vec<std::net::IpAddr> = all_ips
            .into_iter()
            .filter(|ip| !is_ip_blocked_by_policy(*ip, &core_policy))
            .collect();

        if passing.is_empty() {
            return Err(PolicyViolation::PrivateNetwork(host.to_string()));
        }

        let map_value = passing
            .iter()
            .map(std::net::IpAddr::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        Ok(Some(format!(
            "--host-resolver-rules=\"MAP {host} {map_value}\""
        )))
    }

    /// Validate a URL for an **agent-initiated navigation target**: the full
    /// SSRF policy ([`Self::check_url`]) plus, when `block_secrets_in_url` is
    /// set, a scan for embedded credentials. Use this for `goto`/`open`; the
    /// post-navigation active-URL re-check stays on [`Self::check_url`] so a
    /// landed page whose URL legitimately carries a token is still readable.
    pub async fn check_navigation(&self, url_str: &str) -> Result<(), PolicyViolation> {
        self.check_url(url_str).await?;
        if self.config.block_secrets_in_url {
            if let Some(rule) = super::secret_guard::scan_url_for_secrets(url_str) {
                return Err(PolicyViolation::SecretInUrl(rule));
            }
        }
        Ok(())
    }

    /// Scan `text` about to be typed into a page form (type/fill/select/dialog
    /// prompt) for an embedded credential, when `block_secrets_in_input` is
    /// set. Third leg of the secret-egress boundary, symmetric to
    /// [`Self::check_navigation`]'s URL scan: a `Critical`-severity secret in
    /// the model's context must not be typed into a web form on an otherwise
    /// policy-allowed host. Returns the matched rule name on the first hit, or
    /// `None` when the flag is off or the input is clean.
    #[must_use]
    pub fn check_input(&self, text: &str) -> Option<String> {
        if self.config.block_secrets_in_input {
            super::secret_guard::scan_text_for_secrets(text)
        } else {
            None
        }
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

    #[tokio::test]
    async fn test_blocks_localhost() {
        let policy = BrowserSsrfGuard::default();

        assert!(matches!(
            policy.check_url("http://localhost/path").await,
            Err(PolicyViolation::PrivateNetwork(_))
        ));
        assert!(matches!(
            policy.check_url("http://127.0.0.1:8080/api").await,
            Err(PolicyViolation::PrivateNetwork(_))
        ));
        assert!(matches!(
            policy.check_url("http://[::1]/").await,
            Err(PolicyViolation::PrivateNetwork(_))
        ));
    }

    #[tokio::test]
    async fn test_blocks_private_networks() {
        let policy = BrowserSsrfGuard::default();

        // 10.x.x.x
        assert!(matches!(
            policy.check_url("http://10.0.0.1/").await,
            Err(PolicyViolation::PrivateNetwork(_))
        ));
        // 172.16.x.x
        assert!(matches!(
            policy.check_url("http://172.16.0.1/").await,
            Err(PolicyViolation::PrivateNetwork(_))
        ));
        // 172.31.x.x (upper bound)
        assert!(matches!(
            policy.check_url("http://172.31.255.255/").await,
            Err(PolicyViolation::PrivateNetwork(_))
        ));
        // 192.168.x.x
        assert!(matches!(
            policy.check_url("http://192.168.1.1/").await,
            Err(PolicyViolation::PrivateNetwork(_))
        ));
    }

    #[tokio::test]
    async fn test_allows_public_urls() {
        let _lock = serial_test_lock();
        let _scope = install_resolved("example.com", "8.8.8.8".parse().unwrap());
        let policy = BrowserSsrfGuard::default();

        assert!(policy.check_url("https://example.com/page").await.is_ok());
        assert!(policy.check_url("https://8.8.8.8/dns").await.is_ok());
        assert!(policy.check_url("https://172.32.0.1/").await.is_ok()); // 172.32 is NOT private
    }

    #[tokio::test]
    async fn test_blocked_domain_patterns() {
        let _lock = serial_test_lock();
        let _scope = install_resolved("safe.com", "8.8.8.8".parse().unwrap());
        let policy = BrowserSsrfGuard::new(SsrfConfig {
            block_private: false,
            blocked_domains: vec!["*.malware.com".to_string(), "evil.org".to_string()],
            allowed_domains: vec![],
            block_secrets_in_url: false,
            block_secrets_in_input: false,
            redact_secrets_in_content: false,
        });

        // Subdomain match
        assert!(
            policy
                .check_url("https://payload.malware.com/x")
                .await
                .is_err(),
            "subdomain of blocked wildcard should be blocked"
        );
        // Bare domain match for wildcard
        assert!(
            policy.check_url("https://malware.com/x").await.is_err(),
            "bare domain of blocked wildcard should be blocked"
        );
        // Exact match
        assert!(
            policy.check_url("https://evil.org/").await.is_err(),
            "exact blocked domain should be blocked"
        );
        // Non-matching domain is fine
        assert!(policy.check_url("https://safe.com/").await.is_ok());
    }

    #[tokio::test]
    async fn test_allowed_domains_whitelist() {
        let _lock = serial_test_lock();
        // Every host this test navigates to needs a resolver override, not just
        // the rejected one: `check_url` resolves before the allowlist gate, so a
        // host left to real DNS turns "is it allowed?" into "does it exist on
        // the internet right now?". `app.trusted.com` / `api.example.org` do not
        // resolve on a CI runner, which failed the two allow assertions below
        // while passing anywhere with a wildcard-answering resolver.
        let _scope = install_resolved_multi(
            [
                ("app.trusted.com", "8.8.8.8"),
                ("api.example.org", "8.8.8.8"),
                ("random.com", "8.8.8.8"),
            ]
            .into_iter()
            .map(|(host, ip)| (host.to_string(), vec![ip.parse().unwrap()]))
            .collect(),
        );
        let policy = BrowserSsrfGuard::new(SsrfConfig {
            block_private: false,
            blocked_domains: vec![],
            allowed_domains: vec!["*.trusted.com".to_string(), "api.example.org".to_string()],
            block_secrets_in_url: false,
            block_secrets_in_input: false,
            redact_secrets_in_content: false,
        });

        // Allowed
        assert!(policy.check_url("https://app.trusted.com/").await.is_ok());
        assert!(policy.check_url("https://api.example.org/v1").await.is_ok());

        // Not in allowlist (after DNS passes, the browser-level allowlist-only
        // gate kicks in for any host not matching `allowed_domains`).
        assert!(matches!(
            policy.check_url("https://random.com/").await,
            Err(PolicyViolation::NotInAllowlist(_))
        ));
    }

    #[tokio::test]
    async fn test_disabled_ssrf_allows_everything() {
        let _lock = serial_test_lock();
        let _scope = install_resolved("example.com", "8.8.8.8".parse().unwrap());
        let policy = BrowserSsrfGuard::new(SsrfConfig {
            block_private: false,
            blocked_domains: vec![],
            allowed_domains: vec![],
            block_secrets_in_url: false,
            block_secrets_in_input: false,
            redact_secrets_in_content: false,
        });

        assert!(policy.check_url("http://localhost/").await.is_ok());
        assert!(policy.check_url("http://10.0.0.1/").await.is_ok());
        assert!(policy.check_url("http://192.168.1.1/").await.is_ok());
        assert!(policy.check_url("https://example.com/").await.is_ok());
    }

    #[tokio::test]
    async fn test_invalid_url() {
        let policy = BrowserSsrfGuard::default();

        assert!(matches!(
            policy.check_url("not-a-url").await,
            Err(PolicyViolation::InvalidUrl(_))
        ));
    }

    #[tokio::test]
    async fn test_rejects_non_http_schemes() {
        // Host-bearing alternate schemes must not slip past the host-only SSRF
        // checks (SSRF-via-alternate-scheme). Disable host blocking so the only
        // thing that can reject these is the scheme guard itself.
        let policy = BrowserSsrfGuard::new(SsrfConfig {
            block_private: false,
            blocked_domains: vec![],
            allowed_domains: vec![],
            block_secrets_in_url: false,
            block_secrets_in_input: false,
            redact_secrets_in_content: false,
        });
        for url in [
            "gopher://internal-host:6379/_data",
            "ftp://files.example.com/etc/passwd",
            "file:///etc/passwd",
            "data:text/html,<script>alert(1)</script>",
        ] {
            assert!(
                matches!(
                    policy.check_url(url).await,
                    Err(PolicyViolation::InvalidUrl(_))
                ),
                "scheme of {url} should be rejected"
            );
        }
        // http/https still pass when host policy is disabled.
        assert!(policy.check_url("http://example.com/").await.is_ok());
        assert!(policy.check_url("https://example.com/").await.is_ok());
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

    #[tokio::test]
    async fn check_navigation_blocks_secret_in_url() {
        let _lock = serial_test_lock();
        let _scope = install_resolved("public.example", "8.8.8.8".parse().unwrap());
        // Default guard has block_secrets_in_url = true.
        let policy = BrowserSsrfGuard::default();
        let url = "https://public.example/?leak=sk-ant-api03-0123456789abcdefghijklmnop";
        // SSRF alone allows the public host…
        assert!(policy.check_url(url).await.is_ok());
        // …but navigation rejects the embedded credential.
        assert!(matches!(
            policy.check_navigation(url).await,
            Err(PolicyViolation::SecretInUrl(_))
        ));
        // Clean public URL still navigates.
        assert!(policy
            .check_navigation("https://public.example/docs")
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn check_navigation_respects_disabled_secret_scan() {
        let policy = BrowserSsrfGuard::new(SsrfConfig {
            block_private: false,
            blocked_domains: vec![],
            allowed_domains: vec![],
            block_secrets_in_url: false,
            block_secrets_in_input: false,
            redact_secrets_in_content: false,
        });
        let url = "https://public.example/?leak=sk-ant-api03-0123456789abcdefghijklmnop";
        assert!(policy.check_navigation(url).await.is_ok());
    }

    #[test]
    fn check_input_scans_form_text_when_enabled() {
        // Default guard has block_secrets_in_input = true.
        let policy = BrowserSsrfGuard::default();
        let input = "token sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789";
        assert_eq!(policy.check_input(input).as_deref(), Some("api_key"));
        // Clean input passes.
        assert!(policy.check_input("alice@example.com").is_none());
        // Flag off → never blocks, even with a credential-shaped value.
        let policy = BrowserSsrfGuard::new(SsrfConfig {
            block_secrets_in_input: false,
            ..SsrfConfig::default()
        });
        assert!(policy.check_input(input).is_none());
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
            block_secrets_in_input: true,
            redact_secrets_in_content: false,
        });
        let page = "API token: sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789 shown";
        let out = policy.redact_content(page);
        assert_eq!(out, page);
        assert!(matches!(out, Cow::Borrowed(_)));
    }

    // --- Hostname→IP DNS rejection (async check_url resolves DNS, blocking
    //     loopback / private / link-local / metadata hosts before Playwright) ---

    use std::sync::Mutex;
    static HOSTNAME_LOCK: Mutex<()> = Mutex::new(());

    fn serial_test_lock() -> std::sync::MutexGuard<'static, ()> {
        HOSTNAME_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn install_resolved_multi(
        map: std::collections::HashMap<String, Vec<std::net::IpAddr>>,
    ) -> crate::security::ssrf::dns::test_hook::ResolverScope {
        crate::security::ssrf::dns::test_hook::ResolverScope::install(map)
    }

    fn install_resolved(
        host: &str,
        ip: std::net::IpAddr,
    ) -> crate::security::ssrf::dns::test_hook::ResolverScope {
        let mut map = std::collections::HashMap::new();
        map.insert(host.to_string(), vec![ip]);
        crate::security::ssrf::dns::test_hook::ResolverScope::install(map)
    }

    // These tests rely on a global test-only resolver hook. They must run
    // serially so concurrent tests don't see each other's resolver state.
    #[tokio::test]
    async fn check_url_blocks_hostname_resolving_to_loopback() {
        let _lock = serial_test_lock();
        let _scope = install_resolved(
            "evil.example",
            std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
        );
        let policy = BrowserSsrfGuard::default();
        let result = policy.check_url("http://evil.example/admin").await;
        assert!(
            matches!(result, Err(PolicyViolation::PrivateNetwork(_))),
            "hostname resolving to 127.0.0.1 must be blocked — got {result:?}"
        );
    }

    #[tokio::test]
    async fn check_url_blocks_hostname_resolving_to_private_10() {
        let _lock = serial_test_lock();
        let _scope = install_resolved("internal.corp", "10.0.0.5".parse().unwrap());
        let policy = BrowserSsrfGuard::default();
        let result = policy.check_url("http://internal.corp/api").await;
        assert!(
            matches!(result, Err(PolicyViolation::PrivateNetwork(_))),
            "RFC1918 10.0.0.0/8 resolution must be blocked — got {result:?}"
        );
    }

    #[tokio::test]
    async fn check_url_blocks_hostname_resolving_to_private_192() {
        let _lock = serial_test_lock();
        let _scope = install_resolved("router.lan", "192.168.0.1".parse().unwrap());
        let policy = BrowserSsrfGuard::default();
        let result = policy.check_url("http://router.lan/admin").await;
        assert!(
            matches!(result, Err(PolicyViolation::PrivateNetwork(_))),
            "RFC1918 192.168.0.0/16 resolution must be blocked — got {result:?}"
        );
    }

    #[tokio::test]
    async fn check_url_blocks_hostname_resolving_to_link_local() {
        let _lock = serial_test_lock();
        let _scope = install_resolved("apipa.host", "169.254.10.20".parse().unwrap());
        let policy = BrowserSsrfGuard::default();
        let result = policy.check_url("http://apipa.host/").await;
        assert!(
            matches!(result, Err(PolicyViolation::PrivateNetwork(_))),
            "link-local 169.254/16 resolution must be blocked — got {result:?}"
        );
    }

    #[tokio::test]
    async fn check_url_blocks_hostname_resolving_to_cloud_metadata() {
        let _lock = serial_test_lock();
        let _scope = install_resolved("aws.example", "169.254.169.254".parse().unwrap());
        let policy = BrowserSsrfGuard::default();
        let result = policy
            .check_url("http://aws.example/latest/meta-data/")
            .await;
        assert!(
            matches!(result, Err(PolicyViolation::PrivateNetwork(_))),
            "cloud-metadata (169.254.169.254) resolution must be blocked — got {result:?}"
        );
    }

    #[tokio::test]
    async fn check_url_blocks_ipv6_loopback_resolution() {
        let _lock = serial_test_lock();
        let _scope = install_resolved("v6.example", "::1".parse().unwrap());
        let policy = BrowserSsrfGuard::default();
        let result = policy.check_url("http://v6.example/").await;
        assert!(
            matches!(result, Err(PolicyViolation::PrivateNetwork(_))),
            "IPv6 loopback ::1 resolution must be blocked — got {result:?}"
        );
    }

    #[tokio::test]
    async fn check_url_blocks_when_any_returned_ip_is_loopback() {
        // A record set mixing a public IP with a loopback must be rejected
        // (TOCTOU floor: if ANY returned IP is blocked, reject entirely).
        let _lock = serial_test_lock();
        let mut map = std::collections::HashMap::new();
        map.insert(
            "mixed.example".to_string(),
            vec!["8.8.8.8".parse().unwrap(), "127.0.0.1".parse().unwrap()],
        );
        let _scope = install_resolved_multi(map);
        let policy = BrowserSsrfGuard::default();
        let result = policy.check_url("http://mixed.example/").await;
        assert!(
            matches!(result, Err(PolicyViolation::PrivateNetwork(_))),
            "mixed A records containing a loopback must be blocked — got {result:?}"
        );
    }

    #[tokio::test]
    async fn check_url_allows_hostname_resolving_to_public_ip() {
        let _lock = serial_test_lock();
        let _scope = install_resolved("good.example", "8.8.8.8".parse().unwrap());
        let policy = BrowserSsrfGuard::default();
        let result = policy.check_url("http://good.example/path").await;
        assert!(
            result.is_ok(),
            "hostname resolving to a public IP must pass — got {result:?}"
        );
    }

    #[tokio::test]
    async fn check_navigation_blocks_hostname_resolving_to_loopback() {
        let _lock = serial_test_lock();
        let _scope = install_resolved(
            "evil.example",
            std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
        );
        let policy = BrowserSsrfGuard::default();
        let result = policy.check_navigation("https://evil.example/admin").await;
        assert!(
            matches!(result, Err(PolicyViolation::PrivateNetwork(_))),
            "navigation guard must inherit DNS resolution — got {result:?}"
        );
    }

    #[tokio::test]
    async fn check_url_dns_resolution_disabled_when_policy_off() {
        // When block_private=false and no allow/blocklists, the policy is
        // disabled entirely — DNS validation is skipped (loopback reachable).
        let _lock = serial_test_lock();
        let _scope = install_resolved(
            "evil.example",
            std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
        );
        let policy = BrowserSsrfGuard::new(SsrfConfig {
            block_private: false,
            blocked_domains: vec![],
            allowed_domains: vec![],
            block_secrets_in_url: false,
            block_secrets_in_input: false,
            redact_secrets_in_content: false,
        });
        let result = policy.check_url("http://evil.example/admin").await;
        assert!(
            result.is_ok(),
            "with all SSRF gating disabled, even loopback resolution must pass — got {result:?}"
        );
    }

    // --- DNS pinning for Chrome launch (defense against DNS rebinding
    //     between check_url time and Chrome's own resolver time) ---

    #[tokio::test]
    async fn pin_host_resolver_args_returns_none_for_ip_literal_url() {
        // IP literal: Chrome resolves the address directly, no MAP rule needed.
        let policy = BrowserSsrfGuard::default();
        let result = policy
            .pin_host_resolver_args("http://8.8.8.8/path")
            .await
            .expect("IP literal must not error");
        assert_eq!(
            result, None,
            "IP literal has no hostname to pin (Chrome resolves the literal directly)"
        );
    }

    #[tokio::test]
    async fn pin_host_resolver_args_returns_none_when_ssrf_policy_disabled() {
        // When SSRF is disabled we cannot validate any IPs, so we skip pinning
        // rather than hand Chrome an unvalidated MAP rule.
        let _lock = serial_test_lock();
        let _scope = install_resolved("anything.example", "127.0.0.1".parse().unwrap());
        let policy = BrowserSsrfGuard::new(SsrfConfig {
            block_private: false,
            blocked_domains: vec![],
            allowed_domains: vec![],
            block_secrets_in_url: false,
            block_secrets_in_input: false,
            redact_secrets_in_content: false,
        });
        let result = policy
            .pin_host_resolver_args("http://anything.example/")
            .await
            .expect("disabled policy must not error");
        assert_eq!(result, None, "disabled SSRF → no DNS pinning");
    }

    #[tokio::test]
    async fn pin_host_resolver_args_blocks_hostname_resolving_to_loopback() {
        // Same DNS rejection floor as check_url: hostname → 127.0.0.1 is refused
        // before any MAP rule is built.
        let _lock = serial_test_lock();
        let _scope = install_resolved(
            "evil.example",
            std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
        );
        let policy = BrowserSsrfGuard::default();
        let result = policy
            .pin_host_resolver_args("http://evil.example/admin")
            .await;
        assert!(
            matches!(result, Err(PolicyViolation::PrivateNetwork(_))),
            "hostname resolving to 127.0.0.1 must be blocked — got {result:?}"
        );
    }

    #[tokio::test]
    async fn pin_host_resolver_args_blocks_hostname_resolving_to_private_10() {
        let _lock = serial_test_lock();
        let _scope = install_resolved("internal.corp", "10.0.0.5".parse().unwrap());
        let policy = BrowserSsrfGuard::default();
        let result = policy
            .pin_host_resolver_args("http://internal.corp/api")
            .await;
        assert!(
            matches!(result, Err(PolicyViolation::PrivateNetwork(_))),
            "RFC1918 10.0.0.0/8 resolution must be blocked — got {result:?}"
        );
    }

    #[tokio::test]
    async fn pin_host_resolver_args_blocks_hostname_resolving_to_cloud_metadata() {
        let _lock = serial_test_lock();
        let _scope = install_resolved("aws.example", "169.254.169.254".parse().unwrap());
        let policy = BrowserSsrfGuard::default();
        let result = policy
            .pin_host_resolver_args("http://aws.example/latest/meta-data/")
            .await;
        assert!(
            matches!(result, Err(PolicyViolation::PrivateNetwork(_))),
            "cloud-metadata resolution must be blocked — got {result:?}"
        );
    }

    #[tokio::test]
    async fn pin_host_resolver_args_returns_map_arg_for_public_hostname() {
        let _lock = serial_test_lock();
        let _scope = install_resolved("good.example", "8.8.8.8".parse().unwrap());
        let policy = BrowserSsrfGuard::default();
        let result = policy
            .pin_host_resolver_args("https://good.example/path")
            .await
            .expect("public hostname must produce a MAP arg");
        let arg = result.expect("hostname should produce MAP arg");
        assert!(arg.starts_with("--host-resolver-rules="), "arg = {arg}");
        assert!(arg.contains("MAP good.example"), "arg = {arg}");
        assert!(arg.contains("8.8.8.8"), "arg = {arg}");
    }

    #[tokio::test]
    async fn pin_host_resolver_args_lists_all_public_ips_for_multi_a_record_hostname() {
        // Chrome's --host-resolver-rules accepts comma-separated IPs and
        // round-robins them; include every passing IP so all valid resolution
        // paths stay reachable.
        let _lock = serial_test_lock();
        let mut map = std::collections::HashMap::new();
        map.insert(
            "multi.example".to_string(),
            vec!["8.8.8.8".parse().unwrap(), "1.1.1.1".parse().unwrap()],
        );
        let _scope = install_resolved_multi(map);
        let policy = BrowserSsrfGuard::default();
        let result = policy
            .pin_host_resolver_args("http://multi.example/")
            .await
            .expect("public multi-A hostname must produce a MAP arg");
        let arg = result.expect("hostname should produce MAP arg");
        assert!(arg.contains("8.8.8.8"), "arg = {arg}");
        assert!(arg.contains("1.1.1.1"), "arg = {arg}");
        assert!(
            arg.contains("8.8.8.8, 1.1.1.1") || arg.contains("1.1.1.1, 8.8.8.8"),
            "IPs must be comma-separated — arg = {arg}"
        );
    }

    #[tokio::test]
    async fn pin_host_resolver_args_rejects_non_http_scheme() {
        // Scheme floor mirrors check_url: gopher:// must not produce a MAP.
        let policy = BrowserSsrfGuard::default();
        let result = policy
            .pin_host_resolver_args("gopher://internal:6379/x")
            .await;
        assert!(
            matches!(result, Err(PolicyViolation::InvalidUrl(_))),
            "non-http scheme must be rejected — got {result:?}"
        );
    }

    #[tokio::test]
    async fn pin_host_resolver_args_rejects_when_all_resolved_ips_are_loopback() {
        // Residual TOCTOU scenario: between check_url and Chrome launch, an
        // attacker flips a hostname's A records to loopback. The second DNS
        // lookup catches it, every IP fails is_ip_blocked_by_policy, and we
        // surface a PrivateNetwork violation rather than handing Chrome an
        // empty/useless MAP. (Full rebinding coverage is a platform concern —
        // see chrome_launch_args_omits_pin_when_chrome_already_running.)
        let _lock = serial_test_lock();
        let mut map = std::collections::HashMap::new();
        map.insert(
            "rebinding.example".to_string(),
            vec!["127.0.0.1".parse().unwrap(), "127.0.0.2".parse().unwrap()],
        );
        let _scope = install_resolved_multi(map);
        let policy = BrowserSsrfGuard::default();
        let result = policy
            .pin_host_resolver_args("http://rebinding.example/")
            .await;
        assert!(
            matches!(result, Err(PolicyViolation::PrivateNetwork(_))),
            "all-loopback rebinding must surface as PrivateNetwork — got {result:?}"
        );
    }
}
