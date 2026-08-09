//! Hostname allowlist matcher used by the managed proxy.
//!
//! Supports two pattern forms:
//! - Exact match: `api.example.com` matches only `api.example.com`.
//! - Wildcard suffix: `*.example.com` matches any single-label child
//!   (`x.example.com`, `y.example.com`) but **not** the apex `example.com`
//!   itself nor multi-label descendants like `a.b.example.com` (browser-cookie
//!   semantics — single label only).
//!
//! Matching is case-insensitive (DNS labels are case-insensitive per RFC 1035).
//! IP literals (IPv4 / IPv6) compare byte-for-byte after lowercasing the
//! hex digits.

use std::sync::Arc;

#[derive(Debug, Clone, Default)]
pub struct AllowList {
    /// Exact hostname matches (lowercased).
    exact: Vec<Arc<str>>,
    /// Wildcard suffixes — stored without the leading `*.` (lowercased).
    /// `*.example.com` is stored as `example.com` and matches `*.example.com`
    /// (one extra label) but not `example.com` itself.
    wildcard_suffix: Vec<Arc<str>>,
}

impl AllowList {
    /// Build an allowlist from a slice of hostname patterns. Empty patterns
    /// are silently ignored; non-empty patterns are normalised to lowercase.
    pub fn new<I, S>(patterns: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut exact = Vec::new();
        let mut wildcard_suffix = Vec::new();
        for raw in patterns {
            let p = raw.as_ref().trim().to_ascii_lowercase();
            if p.is_empty() {
                continue;
            }
            if let Some(suffix) = p.strip_prefix("*.") {
                if !suffix.is_empty() {
                    wildcard_suffix.push(Arc::<str>::from(suffix));
                }
            } else {
                exact.push(Arc::<str>::from(p));
            }
        }
        Self {
            exact,
            wildcard_suffix,
        }
    }

    /// True if the allowlist permits the given hostname. Hostname is compared
    /// case-insensitively against every pattern.
    #[must_use]
    pub fn permits(&self, host: &str) -> bool {
        let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
        if host.is_empty() {
            return false;
        }
        if self.exact.iter().any(|p| p.as_ref() == host) {
            return true;
        }
        self.wildcard_suffix.iter().any(|suffix| {
            // Single-label child only: "x.example.com" matches "example.com"
            // wildcard, but "a.b.example.com" and "example.com" itself do not.
            host.ends_with(suffix.as_ref())
                && host.len() > suffix.len() + 1
                && host.as_bytes()[host.len() - suffix.len() - 1] == b'.'
                && !host[..host.len() - suffix.len() - 1].contains('.')
        })
    }

    /// True if the allowlist permits nothing.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.exact.is_empty() && self.wildcard_suffix.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_allowlist_permits_nothing() {
        let al = AllowList::default();
        assert!(!al.permits("example.com"));
        assert!(al.is_empty());
    }

    #[test]
    fn exact_match_case_insensitive() {
        let al = AllowList::new(["api.example.com"]);
        assert!(al.permits("api.example.com"));
        assert!(al.permits("API.EXAMPLE.COM"));
        assert!(!al.permits("other.example.com"));
        assert!(!al.permits("example.com"));
    }

    #[test]
    fn trailing_dot_is_stripped() {
        let al = AllowList::new(["api.example.com"]);
        assert!(al.permits("api.example.com."));
    }

    #[test]
    fn wildcard_matches_one_label_below() {
        let al = AllowList::new(["*.example.com"]);
        assert!(al.permits("api.example.com"));
        assert!(al.permits("x.example.com"));
    }

    #[test]
    fn wildcard_does_not_match_apex() {
        let al = AllowList::new(["*.example.com"]);
        assert!(!al.permits("example.com"));
    }

    #[test]
    fn wildcard_does_not_match_two_labels_below() {
        // Cookie-style semantics: single label only.
        let al = AllowList::new(["*.example.com"]);
        assert!(!al.permits("a.b.example.com"));
    }

    #[test]
    fn ip_literal_exact_match() {
        let al = AllowList::new(["140.82.114.4"]);
        assert!(al.permits("140.82.114.4"));
        assert!(!al.permits("140.82.114.5"));
    }

    #[test]
    fn whitespace_and_empty_patterns_ignored() {
        let al = AllowList::new(["  ", "", "example.com"]);
        assert!(al.permits("example.com"));
    }

    #[test]
    fn mixed_exact_and_wildcard() {
        let al = AllowList::new(["github.com", "*.githubusercontent.com"]);
        assert!(al.permits("github.com"));
        assert!(al.permits("raw.githubusercontent.com"));
        assert!(!al.permits("evil.com"));
    }

    #[test]
    fn bare_wildcard_is_ignored() {
        // `*.` alone (empty suffix) would match anything; refuse it.
        let al = AllowList::new(["*."]);
        assert!(al.is_empty());
        assert!(!al.permits("anything.com"));
    }
}
