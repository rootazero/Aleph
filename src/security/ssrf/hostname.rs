//! Hostname validation for SSRF protection.
//!
//! Detects blocked hostnames, legacy IP literal encodings (hex, octal, decimal),
//! and URL credential injection.

/// Canonicalizes a hostname for security comparison.
///
/// Lowercases (case-insensitive DNS) and strips trailing dots. A fully qualified
/// hostname like `evil.com.` (or `evil.com...`) is resolved identically to
/// `evil.com` by the DNS layer but string-compares differently against an
/// allow/deny list — so without this normalization a single trailing dot
/// bypasses every host-based check (the `blocked_hosts` denylist has no IP
/// backstop). Stripping is semantically lossless and therefore safe in both
/// directions (allow and deny).
fn normalize_host(hostname: &str) -> String {
    hostname.to_lowercase().trim_end_matches('.').to_string()
}

/// Returns true if the hostname is on the hardcoded blocklist.
///
/// Blocked: "localhost", "localhost.localdomain", "metadata.google.internal",
/// "metadata.internal", and suffixes ".localhost", ".local", ".internal".
///
/// Also checks the Unicode-decoded form of IDNA/punycode hostnames to prevent
/// homograph attacks (e.g., `localhоst` with Cyrillic `о`).
pub(crate) fn is_blocked_hostname(hostname: &str) -> bool {
    let lower = normalize_host(hostname);
    if check_blocked(&lower) {
        return true;
    }
    // If the hostname looks like punycode, decode it and normalize homoglyphs
    // to catch homograph attacks that bypass the ASCII blocklist.
    if lower.contains("xn--") {
        let unicode = url::quirks::domain_to_unicode(&lower);
        let unicode_lower = unicode.to_lowercase();
        if check_blocked(&unicode_lower) {
            return true;
        }
        // Also normalize homoglyphs (e.g., Cyrillic о → Latin o) and check again
        let normalized = crate::security::content_sanitizer::normalize_homoglyphs(&unicode_lower);
        if check_blocked(&normalized) {
            return true;
        }
    }
    false
}

fn check_blocked(lower: &str) -> bool {
    // Exact matches
    if matches!(
        lower,
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
    let hostname_lower = normalize_host(hostname);
    for pattern in allowed_hosts {
        let pattern_lower = normalize_host(pattern);
        if let Some(base) = pattern_lower.strip_prefix("*.") {
            // strip "*."
            if hostname_lower == base || hostname_lower.ends_with(&format!(".{base}")) {
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

    is_hex_ip_literal(&lower)
        || is_decimal_ip_literal(&lower)
        || is_octal_or_short_ipv4(&lower)
}

fn is_hex_ip_literal(s: &str) -> bool {
    s.starts_with("0x")
        && s.len() > 2
        && s[2..].chars().all(|c| c.is_ascii_hexdigit())
}

fn is_decimal_ip_literal(s: &str) -> bool {
    if s.contains('.') || s.len() <= 3 || !s.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    // Must parse as u32 (max IPv4 decimal form is 4294967295) — otherwise a
    // numeric string >10 digits cannot be an IPv4 literal and is just an
    // unusual all-digit hostname (which the OS DNS layer handles normally).
    s.parse::<u32>().is_ok()
}

fn is_octal_or_short_ipv4(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    if !(2..=4).contains(&parts.len()) {
        return false;
    }

    let all_numeric = || {
        parts
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
    };

    // Octal-encoded IPv4 (e.g., 0177.0.0.1): exactly 4 parts, all numeric,
    // and at least one part is a multi-digit leading-zero value (the actual
    // octal marker — leading zeros are valid in octal notation and can have
    // arbitrary length, e.g. "0177" ≡ octal 177 = 127). 2-3 part hostnames
    // with one leading-zero part (e.g., "0123.com") are legitimate domain
    // names — fall through to the short-form check below. Plain decimal IPv4
    // like "8.8.8.8" has no leading-zero parts and is correctly rejected here
    // so the caller can route it through normal IPv4 validation.
    let has_octal_part = parts
        .iter()
        .any(|p| p.len() > 1 && p.starts_with('0'));
    if parts.len() == 4 && all_numeric() && has_octal_part {
        return true;
    }

    // Short-form IPv4: fewer than 4 dot-separated parts but looks numeric
    // e.g., "127.1" → resolves to 127.0.0.1 on many systems
    if parts.len() < 4 && all_numeric() {
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

    #[test]
    fn blocks_idna_homograph_localhost() {
        // Cyrillic 'о' (U+043E) instead of Latin 'o' — parsed as punycode by url crate.
        // The decoded Unicode form must still match the blocklist.
        assert!(
            is_blocked_hostname("xn--localhst-sbh"),
            "punycode form of Cyrillic homograph should be blocked"
        );
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
    fn rejects_empty_hex_prefix() {
        assert!(!is_legacy_ip_literal("0x"));
        assert!(!is_legacy_ip_literal("0X"));
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

    // --- Trailing-dot normalization (SSRF allow/deny bypass) ---

    #[test]
    fn blocks_localhost_with_trailing_dot() {
        // A single trailing dot is a fully qualified name resolving identically
        // to `localhost` — it must not bypass the hardcoded blocklist.
        assert!(is_blocked_hostname("localhost."));
        assert!(is_blocked_hostname("LOCALHOST."));
        assert!(is_blocked_hostname("localhost.localdomain."));
    }

    #[test]
    fn blocks_metadata_with_trailing_dots() {
        assert!(is_blocked_hostname("metadata.google.internal."));
        // Repeated trailing dots must also be stripped.
        assert!(is_blocked_hostname("metadata.google.internal..."));
    }

    #[test]
    fn blocks_suffix_with_trailing_dot() {
        assert!(is_blocked_hostname("printer.local."));
        assert!(is_blocked_hostname("service.internal."));
        assert!(is_blocked_hostname("evil.localhost."));
    }

    #[test]
    fn blocks_idna_homograph_with_trailing_dot() {
        // Punycode homograph + trailing dot must still resolve to the blocklist.
        assert!(is_blocked_hostname("xn--localhst-sbh."));
    }

    #[test]
    fn blocklist_bypassed_by_trailing_dot_is_closed() {
        // The user `blocked_hosts` denylist has no IP backstop, so a trailing
        // dot must not evade it.
        let hosts = vec!["evil.com".to_string()];
        assert!(is_blocklisted("evil.com.", &hosts));
        assert!(is_blocklisted("evil.com...", &hosts));
        assert!(is_blocklisted("EVIL.COM.", &hosts));
    }

    #[test]
    fn blocklist_pattern_with_trailing_dot_still_matches() {
        // A config entry written as `evil.com.` should behave like `evil.com`.
        let hosts = vec!["evil.com.".to_string()];
        assert!(is_blocklisted("evil.com", &hosts));
        assert!(is_blocklisted("evil.com.", &hosts));
    }

    #[test]
    fn blocklist_wildcard_with_trailing_dot() {
        let hosts = vec!["*.evil.com".to_string()];
        assert!(is_blocklisted("sub.evil.com.", &hosts));
        assert!(is_blocklisted("evil.com.", &hosts));
    }

    #[test]
    fn allowlist_trailing_dot_still_allowed() {
        // Legitimate fully qualified requests must still match the allowlist
        // (fail-closed direction preserved).
        let hosts = vec!["api.example.com".to_string()];
        assert!(is_allowlisted("api.example.com.", &hosts));
        let wild = vec!["*.example.com".to_string()];
        assert!(is_allowlisted("sub.example.com.", &wild));
        assert!(is_allowlisted("example.com.", &wild));
    }
}
