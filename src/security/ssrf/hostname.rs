//! Hostname validation for SSRF protection.
//!
//! Detects blocked hostnames, legacy IP literal encodings (hex, octal, decimal),
//! and URL credential injection.

/// Returns true if the hostname is on the hardcoded blocklist.
///
/// Blocked: "localhost", "localhost.localdomain", "metadata.google.internal",
/// "metadata.internal", and suffixes ".localhost", ".local", ".internal".
pub(crate) fn is_blocked_hostname(hostname: &str) -> bool {
    let lower = hostname.to_lowercase();
    // Exact matches
    if matches!(
        lower.as_str(),
        "localhost" | "localhost.localdomain" | "metadata.google.internal" | "metadata.internal"
    ) {
        return true;
    }
    // Suffix matches
    if lower.ends_with(".localhost") || lower.ends_with(".local") || lower.ends_with(".internal") {
        return true;
    }
    false
}

/// Returns true if the hostname matches any entry in the allowlist.
///
/// Supports case-insensitive exact match and wildcard subdomain patterns
/// (e.g., "*.example.com" matches "example.com" and "sub.example.com").
pub(crate) fn is_allowlisted(hostname: &str, allowed_hosts: &[String]) -> bool {
    let hostname_lower = hostname.to_lowercase();
    for pattern in allowed_hosts {
        let pattern_lower = pattern.to_lowercase();
        if let Some(base) = pattern_lower.strip_prefix("*.") {
            // strip "*."
            if hostname_lower == base || hostname_lower.ends_with(&format!(".{}", base)) {
                return true;
            }
        } else if hostname_lower == pattern_lower {
            return true;
        }
    }
    false
}

/// Returns true if the hostname matches any entry in the blocklist.
///
/// Same matching rules as the allowlist (exact + wildcard).
pub(crate) fn is_blocklisted(hostname: &str, blocked_hosts: &[String]) -> bool {
    // Same matching logic as allowlist
    is_allowlisted(hostname, blocked_hosts)
}

/// Returns true if the hostname is a legacy IP literal encoding that could
/// bypass naive IP checks.
///
/// Detects: hex (0x7f000001), octal (0177.0.0.1), decimal integer (2130706433),
/// short-form IPv4 (127.1).
pub(crate) fn is_legacy_ip_literal(hostname: &str) -> bool {
    let lower = hostname.to_lowercase();

    // Hex IP: starts with 0x and rest is hex digits
    if lower.starts_with("0x") && lower[2..].chars().all(|c| c.is_ascii_hexdigit()) {
        return true;
    }

    // Decimal integer: all digits, more than 3 digits, no dots
    // (normal IPs like "8.8.8.8" have dots; "2130706433" is suspicious)
    if !lower.contains('.') && lower.len() > 3 && lower.chars().all(|c| c.is_ascii_digit()) {
        return true;
    }

    // Octal: any component starts with leading zero (e.g., 0177.0.0.1)
    let parts: Vec<&str> = lower.split('.').collect();
    if parts.len() >= 2 && parts.len() <= 4 {
        for part in &parts {
            if part.len() > 1 && part.starts_with('0') && part.chars().all(|c| c.is_ascii_digit()) {
                return true;
            }
        }
    }

    // Short-form IPv4: fewer than 4 dot-separated parts but looks numeric
    // e.g., "127.1" → resolves to 127.0.0.1 on many systems
    if parts.len() >= 2
        && parts.len() < 4
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
    {
        return true;
    }

    false
}

/// Returns true if the URL string contains embedded credentials (user:pass@host).
pub(crate) fn has_url_credentials(url_str: &str) -> bool {
    match url::Url::parse(url_str) {
        Ok(parsed) => !parsed.username().is_empty() || parsed.password().is_some(),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Blocked hostnames ---

    #[test]
    fn blocks_localhost() {
        assert!(is_blocked_hostname("localhost"));
        assert!(is_blocked_hostname("LOCALHOST"));
        assert!(is_blocked_hostname("Localhost"));
    }

    #[test]
    fn blocks_localhost_localdomain() {
        assert!(is_blocked_hostname("localhost.localdomain"));
    }

    #[test]
    fn blocks_metadata_hostnames() {
        assert!(is_blocked_hostname("metadata.google.internal"));
        assert!(is_blocked_hostname("metadata.internal"));
    }

    #[test]
    fn blocks_localhost_suffix() {
        assert!(is_blocked_hostname("evil.localhost"));
        assert!(is_blocked_hostname("sub.evil.localhost"));
    }

    #[test]
    fn blocks_local_suffix() {
        assert!(is_blocked_hostname("printer.local"));
        assert!(is_blocked_hostname("myhost.local"));
    }

    #[test]
    fn blocks_internal_suffix() {
        assert!(is_blocked_hostname("service.internal"));
        assert!(is_blocked_hostname("deep.nested.internal"));
    }

    #[test]
    fn allows_public_hostname() {
        assert!(!is_blocked_hostname("example.com"));
        assert!(!is_blocked_hostname("api.github.com"));
    }

    // --- Allowlist matching ---

    #[test]
    fn allowlist_exact_match() {
        let hosts = vec!["api.example.com".to_string()];
        assert!(is_allowlisted("api.example.com", &hosts));
        assert!(is_allowlisted("API.EXAMPLE.COM", &hosts));
        assert!(!is_allowlisted("other.com", &hosts));
    }

    #[test]
    fn allowlist_wildcard_match() {
        let hosts = vec!["*.example.com".to_string()];
        assert!(is_allowlisted("api.example.com", &hosts));
        assert!(is_allowlisted("sub.api.example.com", &hosts));
        assert!(is_allowlisted("example.com", &hosts));
        assert!(!is_allowlisted("example.org", &hosts));
    }

    // --- Blocklist matching ---

    #[test]
    fn blocklist_exact_match() {
        let hosts = vec!["evil.com".to_string()];
        assert!(is_blocklisted("evil.com", &hosts));
        assert!(!is_blocklisted("good.com", &hosts));
    }

    #[test]
    fn blocklist_wildcard_match() {
        let hosts = vec!["*.evil.com".to_string()];
        assert!(is_blocklisted("sub.evil.com", &hosts));
        assert!(is_blocklisted("evil.com", &hosts));
    }

    // --- Legacy IP literals ---

    #[test]
    fn detects_hex_ip() {
        assert!(is_legacy_ip_literal("0x7f000001"));
        assert!(is_legacy_ip_literal("0xC0A80101"));
    }

    #[test]
    fn detects_octal_ip() {
        assert!(is_legacy_ip_literal("0177.0.0.1"));
        assert!(is_legacy_ip_literal("010.0.0.1"));
    }

    #[test]
    fn detects_decimal_integer_ip() {
        assert!(is_legacy_ip_literal("2130706433"));
        assert!(is_legacy_ip_literal("3232235777"));
    }

    #[test]
    fn detects_short_form_ip() {
        assert!(is_legacy_ip_literal("127.1"));
        assert!(is_legacy_ip_literal("10.1"));
    }

    #[test]
    fn allows_normal_hostname() {
        assert!(!is_legacy_ip_literal("example.com"));
        assert!(!is_legacy_ip_literal("api.github.com"));
    }

    #[test]
    fn allows_normal_ipv4() {
        // Standard dotted-quad with 4 parts, no leading zeros
        assert!(!is_legacy_ip_literal("8.8.8.8"));
        assert!(!is_legacy_ip_literal("192.168.1.1"));
    }

    // --- URL credentials ---

    #[test]
    fn detects_url_with_username() {
        assert!(has_url_credentials("http://admin@example.com/"));
    }

    #[test]
    fn detects_url_with_password() {
        assert!(has_url_credentials("http://user:pass@example.com/"));
    }

    #[test]
    fn no_credentials_in_normal_url() {
        assert!(!has_url_credentials("https://example.com/path"));
    }
}
