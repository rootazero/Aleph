# SSRF Security Engine Upgrade Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Unify Aleph's two separate SSRF implementations into a single hardened engine with DNS pinning, redirect chain validation, full IP range coverage, and Panel-configurable security settings.

**Architecture:** Replace `src/security/ssrf.rs` (single file) with `src/security/ssrf/` (module directory). The module provides `policy.rs`, `ip.rs`, `hostname.rs`, `dns.rs`, `fetch.rs`, and `mod.rs`. All outbound HTTP requests route through `safe_fetch()`. Browser module becomes a thin wrapper. Panel UI adds an Outbound Security section.

**Tech Stack:** Rust (tokio, reqwest, url crate), Leptos (WASM Panel UI)

**Spec:** `docs/superpowers/specs/2026-03-31-ssrf-security-engine-upgrade-design.md`

---

## File Structure

### Create
- `src/security/ssrf/mod.rs` — Public API re-exports
- `src/security/ssrf/policy.rs` — SsrfPolicy struct + defaults
- `src/security/ssrf/ip.rs` — IP classification (IPv4/IPv6/embedded)
- `src/security/ssrf/hostname.rs` — Hostname blocklist/allowlist/glob
- `src/security/ssrf/dns.rs` — DNS resolve + validate + pinning
- `src/security/ssrf/fetch.rs` — safe_fetch() with redirect chain validation

### Modify
- `src/security/mod.rs` — Update ssrf module declaration
- `src/builtin_tools/web_fetch.rs` — Migrate to safe_fetch()
- `src/tasks/cron/webhook_target.rs` — Add SSRF via safe_fetch()
- `src/gateway/pipeline/media_download.rs` — Migrate to safe_fetch()
- `src/mcp/transport/http.rs` — Migrate to validate_url (new module path)
- `src/browser/network_policy.rs` — Thin wrapper over ssrf engine
- `src/browser/manager.rs` — Update import paths
- `src/gateway/handlers/security_config.rs` — Add SSRF config read/write
- `interfaces/webchat/src/api/security.rs` — Add SSRF fields to SecurityConfig
- `interfaces/webchat/src/views/settings/security.rs` — Add OutboundSecuritySection

### Delete
- `src/security/ssrf.rs` — Replaced by ssrf/ directory module

---

## Task 1: Create policy.rs — SsrfPolicy struct

**Files:**
- Create: `src/security/ssrf/policy.rs`

- [ ] **Step 1: Write failing test for SsrfPolicy defaults**

```rust
// src/security/ssrf/policy.rs

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_blocks_private_network() {
        let policy = SsrfPolicy::default();
        assert!(!policy.allow_private_network);
        assert!(policy.allowed_hosts.is_empty());
        assert!(policy.blocked_hosts.is_empty());
        assert_eq!(policy.max_redirects, 5);
        assert!(policy.strip_auth_on_cross_origin);
        assert!(policy.enabled);
    }

    #[test]
    fn disabled_policy() {
        let policy = SsrfPolicy::disabled();
        assert!(!policy.enabled);
    }
}
```

- [ ] **Step 2: Implement SsrfPolicy**

```rust
// src/security/ssrf/policy.rs

use serde::{Deserialize, Serialize};

/// Policy controlling SSRF validation behavior.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SsrfPolicy {
    /// Master switch. When false, all SSRF checks are skipped.
    pub enabled: bool,
    /// Whether to allow requests to private/internal IP ranges.
    /// Even when true, loopback and cloud metadata remain blocked.
    pub allow_private_network: bool,
    /// Hosts that bypass the blocklist. Supports exact matches
    /// (e.g., "api.example.com") and wildcard subdomain matches (e.g., "*.example.com").
    pub allowed_hosts: Vec<String>,
    /// Hosts to block. Supports glob patterns (e.g., "*.malware.com").
    pub blocked_hosts: Vec<String>,
    /// Maximum redirect hops (default: 5).
    pub max_redirects: u8,
    /// Strip Authorization/Cookie headers on cross-origin redirects (default: true).
    pub strip_auth_on_cross_origin: bool,
}

impl Default for SsrfPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            allow_private_network: false,
            allowed_hosts: Vec::new(),
            blocked_hosts: Vec::new(),
            max_redirects: 5,
            strip_auth_on_cross_origin: true,
        }
    }
}

impl SsrfPolicy {
    /// Create a disabled policy that skips all SSRF checks.
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            ..Default::default()
        }
    }
}
```

- [ ] **Step 3: Run test to verify it passes**

Run: `cargo test -p alephcore --lib ssrf::policy -- --nocapture`
Expected: 2 tests PASS

- [ ] **Step 4: Commit**

```bash
git add src/security/ssrf/policy.rs
git commit -m "security: add SsrfPolicy struct with configurable fields"
```

---

## Task 2: Create ip.rs — Full IP classification

**Files:**
- Create: `src/security/ssrf/ip.rs`

- [ ] **Step 1: Write failing tests for all IP ranges**

```rust
// src/security/ssrf/ip.rs — tests at bottom

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    // --- IPv4 private ranges ---

    #[test]
    fn blocks_loopback() {
        assert!(is_blocked_ipv4(Ipv4Addr::new(127, 0, 0, 1)));
        assert!(is_blocked_ipv4(Ipv4Addr::new(127, 255, 255, 255)));
    }

    #[test]
    fn blocks_rfc1918_10() {
        assert!(is_blocked_ipv4(Ipv4Addr::new(10, 0, 0, 1)));
        assert!(is_blocked_ipv4(Ipv4Addr::new(10, 255, 255, 255)));
    }

    #[test]
    fn blocks_rfc1918_172() {
        assert!(is_blocked_ipv4(Ipv4Addr::new(172, 16, 0, 1)));
        assert!(is_blocked_ipv4(Ipv4Addr::new(172, 31, 255, 255)));
        // 172.32 is public
        assert!(!is_blocked_ipv4(Ipv4Addr::new(172, 32, 0, 1)));
    }

    #[test]
    fn blocks_rfc1918_192() {
        assert!(is_blocked_ipv4(Ipv4Addr::new(192, 168, 0, 1)));
        assert!(is_blocked_ipv4(Ipv4Addr::new(192, 168, 255, 255)));
    }

    #[test]
    fn blocks_cgnat() {
        assert!(is_blocked_ipv4(Ipv4Addr::new(100, 64, 0, 1)));
        assert!(is_blocked_ipv4(Ipv4Addr::new(100, 127, 255, 255)));
        // 100.128 is public
        assert!(!is_blocked_ipv4(Ipv4Addr::new(100, 128, 0, 1)));
    }

    #[test]
    fn blocks_link_local() {
        assert!(is_blocked_ipv4(Ipv4Addr::new(169, 254, 1, 1)));
        assert!(is_blocked_ipv4(Ipv4Addr::new(169, 254, 169, 254)));
    }

    #[test]
    fn blocks_unspecified() {
        assert!(is_blocked_ipv4(Ipv4Addr::new(0, 0, 0, 0)));
        assert!(is_blocked_ipv4(Ipv4Addr::new(0, 255, 255, 255)));
    }

    #[test]
    fn blocks_test_nets() {
        // TEST-NET-1
        assert!(is_blocked_ipv4(Ipv4Addr::new(192, 0, 2, 1)));
        // TEST-NET-2
        assert!(is_blocked_ipv4(Ipv4Addr::new(198, 51, 100, 1)));
        // TEST-NET-3
        assert!(is_blocked_ipv4(Ipv4Addr::new(203, 0, 113, 1)));
    }

    #[test]
    fn blocks_benchmark() {
        assert!(is_blocked_ipv4(Ipv4Addr::new(198, 18, 0, 1)));
        assert!(is_blocked_ipv4(Ipv4Addr::new(198, 19, 255, 255)));
        // 198.20 is public
        assert!(!is_blocked_ipv4(Ipv4Addr::new(198, 20, 0, 1)));
    }

    #[test]
    fn blocks_multicast() {
        assert!(is_blocked_ipv4(Ipv4Addr::new(224, 0, 0, 1)));
        assert!(is_blocked_ipv4(Ipv4Addr::new(239, 255, 255, 255)));
    }

    #[test]
    fn blocks_reserved() {
        assert!(is_blocked_ipv4(Ipv4Addr::new(240, 0, 0, 1)));
        assert!(is_blocked_ipv4(Ipv4Addr::new(255, 255, 255, 255)));
    }

    #[test]
    fn allows_public_ipv4() {
        assert!(!is_blocked_ipv4(Ipv4Addr::new(8, 8, 8, 8)));
        assert!(!is_blocked_ipv4(Ipv4Addr::new(1, 1, 1, 1)));
        assert!(!is_blocked_ipv4(Ipv4Addr::new(142, 250, 80, 46)));
    }

    // --- IPv6 ranges ---

    #[test]
    fn blocks_ipv6_loopback() {
        assert!(is_blocked_ipv6(Ipv6Addr::LOCALHOST));
    }

    #[test]
    fn blocks_ipv6_unspecified() {
        assert!(is_blocked_ipv6(Ipv6Addr::UNSPECIFIED));
    }

    #[test]
    fn blocks_ipv6_link_local() {
        // fe80::1
        assert!(is_blocked_ipv6(Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1)));
    }

    #[test]
    fn blocks_ipv6_ula() {
        // fc00::1
        assert!(is_blocked_ipv6(Ipv6Addr::new(0xfc00, 0, 0, 0, 0, 0, 0, 1)));
        // fd00::1
        assert!(is_blocked_ipv6(Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 1)));
    }

    #[test]
    fn blocks_ipv6_multicast() {
        // ff02::1
        assert!(is_blocked_ipv6(Ipv6Addr::new(0xff02, 0, 0, 0, 0, 0, 0, 1)));
    }

    #[test]
    fn blocks_ipv4_mapped_ipv6_private() {
        // ::ffff:127.0.0.1
        assert!(is_blocked_ipv6(Ipv6Addr::new(0, 0, 0, 0, 0, 0xffff, 0x7f00, 0x0001)));
        // ::ffff:10.0.0.1
        assert!(is_blocked_ipv6(Ipv6Addr::new(0, 0, 0, 0, 0, 0xffff, 0x0a00, 0x0001)));
    }

    #[test]
    fn blocks_nat64_private() {
        // 64:ff9b::127.0.0.1 = 64:ff9b::7f00:1
        assert!(is_blocked_ipv6(Ipv6Addr::new(0x0064, 0xff9b, 0, 0, 0, 0, 0x7f00, 0x0001)));
    }

    #[test]
    fn blocks_6to4_private() {
        // 2002:7f00:0001:: (embeds 127.0.0.1)
        assert!(is_blocked_ipv6(Ipv6Addr::new(0x2002, 0x7f00, 0x0001, 0, 0, 0, 0, 0)));
        // 2002:0a00:0001:: (embeds 10.0.0.1)
        assert!(is_blocked_ipv6(Ipv6Addr::new(0x2002, 0x0a00, 0x0001, 0, 0, 0, 0, 0)));
    }

    #[test]
    fn blocks_teredo_private() {
        // Teredo: 2001:0000:...:<client_port_xor>:<client_ip_xor>
        // Client IP is XOR'd with 0xffffffff. For 127.0.0.1 → XOR → 0x80ff fffe
        assert!(is_blocked_ipv6(Ipv6Addr::new(0x2001, 0x0000, 0, 0, 0, 0, 0x80ff, 0xfffe)));
    }

    #[test]
    fn allows_public_ipv6() {
        // 2607:f8b0:4004:800::200e (Google)
        assert!(!is_blocked_ipv6(Ipv6Addr::new(0x2607, 0xf8b0, 0x4004, 0x800, 0, 0, 0, 0x200e)));
    }

    // --- Combined is_blocked_ip ---

    #[test]
    fn is_blocked_ip_delegates_correctly() {
        use std::net::IpAddr;
        assert!(is_blocked_ip(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))));
        assert!(is_blocked_ip(IpAddr::V6(Ipv6Addr::LOCALHOST)));
        assert!(!is_blocked_ip(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))));
    }

    // --- Policy-aware check ---

    #[test]
    fn allow_private_still_blocks_loopback_and_metadata() {
        use std::net::IpAddr;
        let policy = SsrfPolicy { allow_private_network: true, ..SsrfPolicy::default() };
        // Private allowed
        assert!(!is_ip_blocked_by_policy(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)), &policy));
        // Loopback still blocked
        assert!(is_ip_blocked_by_policy(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), &policy));
        // Cloud metadata still blocked
        assert!(is_ip_blocked_by_policy(IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254)), &policy));
    }
}
```

- [ ] **Step 2: Implement full IP classification**

```rust
// src/security/ssrf/ip.rs

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use super::policy::SsrfPolicy;

/// Cloud metadata IP address (AWS, GCP, Azure).
const CLOUD_METADATA_IP: Ipv4Addr = Ipv4Addr::new(169, 254, 169, 254);

/// Returns true if the IPv4 address falls in any blocked range.
pub(crate) fn is_blocked_ipv4(ip: Ipv4Addr) -> bool {
    let o = ip.octets();

    // Unspecified: 0.0.0.0/8
    o[0] == 0
    // Loopback: 127.0.0.0/8
    || o[0] == 127
    // RFC1918: 10.0.0.0/8
    || o[0] == 10
    // RFC1918: 172.16.0.0/12
    || (o[0] == 172 && (16..=31).contains(&o[1]))
    // RFC1918: 192.168.0.0/16
    || (o[0] == 192 && o[1] == 168)
    // CGNAT: 100.64.0.0/10
    || (o[0] == 100 && (o[1] & 0xC0) == 64)
    // Link-local: 169.254.0.0/16
    || (o[0] == 169 && o[1] == 254)
    // TEST-NET-1: 192.0.2.0/24
    || (o[0] == 192 && o[1] == 0 && o[2] == 2)
    // Benchmark: 198.18.0.0/15
    || (o[0] == 198 && (o[1] == 18 || o[1] == 19))
    // TEST-NET-2: 198.51.100.0/24
    || (o[0] == 198 && o[1] == 51 && o[2] == 100)
    // TEST-NET-3: 203.0.113.0/24
    || (o[0] == 203 && o[1] == 0 && o[2] == 113)
    // Multicast: 224.0.0.0/4
    || (o[0] & 0xF0) == 224
    // Reserved + broadcast: 240.0.0.0/4
    || (o[0] & 0xF0) == 240
}

/// Extract embedded IPv4 from IPv6 and check if it's blocked.
fn extract_and_check_embedded_ipv4(ip: Ipv6Addr) -> bool {
    let seg = ip.segments();

    // IPv4-mapped: ::ffff:x.x.x.x
    if let Some(v4) = ip.to_ipv4_mapped() {
        return is_blocked_ipv4(v4);
    }

    // NAT64: 64:ff9b::x.x.x.x and 64:ff9b:1::/48
    if seg[0] == 0x0064 && seg[1] == 0xff9b {
        let v4 = Ipv4Addr::new(
            (seg[6] >> 8) as u8, seg[6] as u8,
            (seg[7] >> 8) as u8, seg[7] as u8,
        );
        return is_blocked_ipv4(v4);
    }

    // 6to4: 2002:AABB:CCDD::/48 — embeds IPv4 in segments 1-2
    if seg[0] == 0x2002 {
        let v4 = Ipv4Addr::new(
            (seg[1] >> 8) as u8, seg[1] as u8,
            (seg[2] >> 8) as u8, seg[2] as u8,
        );
        return is_blocked_ipv4(v4);
    }

    // Teredo: 2001:0000:...:<flags>:<port_xor>:<ip_xor>
    // Client IPv4 = last 32 bits XOR 0xFFFFFFFF
    if seg[0] == 0x2001 && seg[1] == 0x0000 {
        let xored_hi = seg[6];
        let xored_lo = seg[7];
        let v4 = Ipv4Addr::new(
            (!xored_hi >> 8) as u8, !xored_hi as u8,
            (!xored_lo >> 8) as u8, !xored_lo as u8,
        );
        return is_blocked_ipv4(v4);
    }

    // IPv4-compatible (deprecated): ::x.x.x.x (segments 0-5 are zero, 6-7 hold IPv4)
    if seg[0..6] == [0, 0, 0, 0, 0, 0] && (seg[6] != 0 || seg[7] > 1) {
        let v4 = Ipv4Addr::new(
            (seg[6] >> 8) as u8, seg[6] as u8,
            (seg[7] >> 8) as u8, seg[7] as u8,
        );
        return is_blocked_ipv4(v4);
    }

    false
}

/// Returns true if the IPv6 address falls in any blocked range.
pub(crate) fn is_blocked_ipv6(ip: Ipv6Addr) -> bool {
    let seg = ip.segments();

    // Loopback: ::1
    ip.is_loopback()
    // Unspecified: ::
    || ip.is_unspecified()
    // Link-local: fe80::/10
    || (seg[0] & 0xFFC0) == 0xFE80
    // Unique local: fc00::/7
    || (seg[0] & 0xFE00) == 0xFC00
    // Multicast: ff00::/8
    || (seg[0] & 0xFF00) == 0xFF00
    // Embedded IPv4 variants
    || extract_and_check_embedded_ipv4(ip)
}

/// Returns true if the IP address should be blocked.
pub(crate) fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_blocked_ipv4(v4),
        IpAddr::V6(v6) => is_blocked_ipv6(v6),
    }
}

/// Policy-aware IP check. When `allow_private_network` is true,
/// only loopback and cloud metadata (169.254.169.254) remain blocked.
pub(crate) fn is_ip_blocked_by_policy(ip: IpAddr, policy: &SsrfPolicy) -> bool {
    if !policy.enabled {
        return false;
    }
    if policy.allow_private_network {
        match ip {
            IpAddr::V4(v4) => v4.is_loopback() || v4 == CLOUD_METADATA_IP,
            IpAddr::V6(v6) => {
                v6.is_loopback()
                    || v6.to_ipv4_mapped().map_or(false, |m| {
                        m.is_loopback() || m == CLOUD_METADATA_IP
                    })
            }
        }
    } else {
        is_blocked_ip(ip)
    }
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p alephcore --lib ssrf::ip -- --nocapture`
Expected: All tests PASS

- [ ] **Step 4: Commit**

```bash
git add src/security/ssrf/ip.rs
git commit -m "security: add comprehensive IP classification with IPv6 embedded IPv4"
```

---

## Task 3: Create hostname.rs — Hostname validation

**Files:**
- Create: `src/security/ssrf/hostname.rs`

- [ ] **Step 1: Write failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_localhost_variants() {
        assert!(is_blocked_hostname("localhost"));
        assert!(is_blocked_hostname("LOCALHOST"));
        assert!(is_blocked_hostname("localhost.localdomain"));
    }

    #[test]
    fn blocks_cloud_metadata_hostnames() {
        assert!(is_blocked_hostname("metadata.google.internal"));
        assert!(is_blocked_hostname("metadata.internal"));
    }

    #[test]
    fn blocks_dangerous_suffixes() {
        assert!(is_blocked_hostname("foo.localhost"));
        assert!(is_blocked_hostname("myapp.local"));
        assert!(is_blocked_hostname("service.internal"));
    }

    #[test]
    fn allows_normal_hostnames() {
        assert!(!is_blocked_hostname("example.com"));
        assert!(!is_blocked_hostname("api.example.com"));
        assert!(!is_blocked_hostname("localhost.example.com")); // localhost is substring, not suffix
    }

    #[test]
    fn allowlist_exact_match() {
        let hosts = vec!["internal.corp.com".to_string()];
        assert!(is_allowlisted("internal.corp.com", &hosts));
        assert!(!is_allowlisted("other.corp.com", &hosts));
    }

    #[test]
    fn allowlist_wildcard_match() {
        let hosts = vec!["*.example.com".to_string()];
        assert!(is_allowlisted("api.example.com", &hosts));
        assert!(is_allowlisted("example.com", &hosts));
        assert!(is_allowlisted("deep.sub.example.com", &hosts));
        assert!(!is_allowlisted("example.org", &hosts));
    }

    #[test]
    fn blocklist_glob_match() {
        let hosts = vec!["*.malware.com".to_string(), "evil.org".to_string()];
        assert!(is_blocklisted("payload.malware.com", &hosts));
        assert!(is_blocklisted("malware.com", &hosts));
        assert!(is_blocklisted("evil.org", &hosts));
        assert!(!is_blocklisted("safe.com", &hosts));
    }

    #[test]
    fn detects_legacy_ipv4_literal() {
        assert!(is_legacy_ip_literal("0177.0.0.1"));      // octal
        assert!(is_legacy_ip_literal("0x7f000001"));       // hex
        assert!(is_legacy_ip_literal("2130706433"));        // decimal
        assert!(is_legacy_ip_literal("127.1"));             // short-form
        assert!(!is_legacy_ip_literal("127.0.0.1"));        // standard form OK
        assert!(!is_legacy_ip_literal("example.com"));      // hostname OK
    }

    #[test]
    fn detects_url_credential_obfuscation() {
        assert!(has_url_credentials("http://evil.com@127.0.0.1:8080/"));
        assert!(has_url_credentials("http://user:pass@internal.host/"));
        assert!(!has_url_credentials("http://example.com/path"));
        assert!(!has_url_credentials("http://example.com/path?user@host"));
    }
}
```

- [ ] **Step 2: Implement hostname validation**

```rust
// src/security/ssrf/hostname.rs

use url::Url;

/// Hardcoded blocked hostnames.
const BLOCKED_EXACT: &[&str] = &[
    "localhost",
    "localhost.localdomain",
    "metadata.google.internal",
    "metadata.internal",
];

/// Blocked hostname suffixes (matched against the end of the hostname).
const BLOCKED_SUFFIXES: &[&str] = &[
    ".localhost",
    ".local",
    ".internal",
];

/// Returns true if the hostname is on the hardcoded blocklist.
pub(crate) fn is_blocked_hostname(hostname: &str) -> bool {
    let lower = hostname.to_ascii_lowercase();
    if BLOCKED_EXACT.iter().any(|&h| lower == h) {
        return true;
    }
    BLOCKED_SUFFIXES.iter().any(|&suffix| lower.ends_with(suffix))
}

/// Returns true if the hostname matches any entry in the allowlist.
/// Supports exact (case-insensitive) and wildcard subdomain (*.example.com).
pub(crate) fn is_allowlisted(hostname: &str, allowed_hosts: &[String]) -> bool {
    let lower = hostname.to_ascii_lowercase();
    for pattern in allowed_hosts {
        let pat = pattern.to_ascii_lowercase();
        if let Some(base) = pat.strip_prefix("*.") {
            if lower == base || lower.ends_with(&format!(".{base}")) {
                return true;
            }
        } else if lower == pat {
            return true;
        }
    }
    false
}

/// Returns true if the hostname matches any entry in the blocklist.
/// Same matching logic as allowlist.
pub(crate) fn is_blocklisted(hostname: &str, blocked_hosts: &[String]) -> bool {
    is_allowlisted(hostname, blocked_hosts) // same glob logic
}

/// Detects non-standard (legacy) IPv4 literal formats that can bypass naive parsing:
/// octal (0177.0.0.1), hex (0x7f000001), decimal (2130706433), short-form (127.1).
pub(crate) fn is_legacy_ip_literal(host: &str) -> bool {
    // Must look like a potential IP, not a hostname with letters
    if host.is_empty() {
        return false;
    }

    // Hex prefix: 0x...
    if host.starts_with("0x") || host.starts_with("0X") {
        return host[2..].chars().all(|c| c.is_ascii_hexdigit());
    }

    // Pure decimal (single number, no dots): e.g., 2130706433
    if host.chars().all(|c| c.is_ascii_digit()) && !host.contains('.') && host.len() > 3 {
        return true;
    }

    // Dotted format: check for octal or short-form
    let parts: Vec<&str> = host.split('.').collect();
    if parts.iter().all(|p| p.chars().all(|c| c.is_ascii_digit())) {
        // Short-form: fewer than 4 parts (e.g., 127.1)
        if parts.len() < 4 && parts.len() >= 2 {
            return true;
        }
        // Octal: any part with leading zero and length > 1 (e.g., 0177)
        if parts.iter().any(|p| p.len() > 1 && p.starts_with('0')) {
            return true;
        }
    }

    false
}

/// Returns true if the URL contains credentials (user@host or user:pass@host).
pub(crate) fn has_url_credentials(url_str: &str) -> bool {
    if let Ok(url) = Url::parse(url_str) {
        !url.username().is_empty() || url.password().is_some()
    } else {
        false
    }
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p alephcore --lib ssrf::hostname -- --nocapture`
Expected: All tests PASS

- [ ] **Step 4: Commit**

```bash
git add src/security/ssrf/hostname.rs
git commit -m "security: add hostname blocklist, allowlist, legacy IP detection"
```

---

## Task 4: Create dns.rs — DNS resolution with pinning

**Files:**
- Create: `src/security/ssrf/dns.rs`

- [ ] **Step 1: Write failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, SocketAddr};

    #[tokio::test]
    async fn resolves_ip_literal_directly() {
        let policy = SsrfPolicy::default();
        let result = resolve_and_validate("8.8.8.8", 443, &policy).await;
        assert!(result.is_ok());
        let addr = result.unwrap();
        assert_eq!(addr.ip(), IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)));
    }

    #[tokio::test]
    async fn blocks_private_ip_literal() {
        let policy = SsrfPolicy::default();
        let result = resolve_and_validate("127.0.0.1", 80, &policy).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn blocks_ipv6_loopback_literal() {
        let policy = SsrfPolicy::default();
        let result = resolve_and_validate("::1", 80, &policy).await;
        assert!(result.is_err());
    }
}
```

- [ ] **Step 2: Implement DNS resolution with validation**

```rust
// src/security/ssrf/dns.rs

use std::net::{IpAddr, SocketAddr};
use super::ip::is_ip_blocked_by_policy;
use super::policy::SsrfPolicy;
use super::SsrfError;

/// Resolve a hostname (or IP literal) and validate all returned addresses.
/// Returns the first valid SocketAddr for DNS pinning.
pub(crate) async fn resolve_and_validate(
    host: &str,
    port: u16,
    policy: &SsrfPolicy,
) -> Result<SocketAddr, SsrfError> {
    // IP literal: validate directly, no DNS needed
    if let Ok(ip) = host.parse::<IpAddr>() {
        if is_ip_blocked_by_policy(ip, policy) {
            return Err(SsrfError::BlockedAddress(ip.to_string()));
        }
        return Ok(SocketAddr::new(ip, port));
    }

    // Also try stripping IPv6 brackets
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = bare.parse::<IpAddr>() {
        if is_ip_blocked_by_policy(ip, policy) {
            return Err(SsrfError::BlockedAddress(ip.to_string()));
        }
        return Ok(SocketAddr::new(ip, port));
    }

    // DNS resolution
    let lookup_addr = format!("{host}:{port}");
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host(&lookup_addr)
        .await
        .map_err(|e| SsrfError::DnsResolutionFailed {
            host: host.to_string(),
            reason: e.to_string(),
        })?
        .collect();

    if addrs.is_empty() {
        return Err(SsrfError::DnsResolutionFailed {
            host: host.to_string(),
            reason: "no addresses returned".to_string(),
        });
    }

    // Validate ALL returned IPs — fail if any is blocked
    for addr in &addrs {
        if is_ip_blocked_by_policy(addr.ip(), policy) {
            return Err(SsrfError::BlockedAddress(addr.ip().to_string()));
        }
    }

    // Return first valid address for pinning
    Ok(addrs[0])
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p alephcore --lib ssrf::dns -- --nocapture`
Expected: All tests PASS

- [ ] **Step 4: Commit**

```bash
git add src/security/ssrf/dns.rs
git commit -m "security: add DNS resolution with validation for pinning"
```

---

## Task 5: Create fetch.rs — safe_fetch with redirect chain validation

**Files:**
- Create: `src/security/ssrf/fetch.rs`

- [ ] **Step 1: Write failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_fetch_request_default() {
        let req = SafeFetchRequest::get(std::time::Duration::from_secs(30));
        assert_eq!(req.method, Method::GET);
        assert!(req.headers.is_empty());
        assert!(req.body.is_none());
    }

    #[test]
    fn rejects_non_http_scheme() {
        let result = validate_scheme("ftp://example.com");
        assert!(result.is_err());
    }

    #[test]
    fn accepts_http_and_https() {
        assert!(validate_scheme("http://example.com").is_ok());
        assert!(validate_scheme("https://example.com").is_ok());
    }

    #[test]
    fn detects_cross_origin() {
        assert!(is_cross_origin("https://a.com/path", "https://b.com/path"));
        assert!(!is_cross_origin("https://a.com/path1", "https://a.com/path2"));
        assert!(is_cross_origin("http://a.com/path", "https://a.com/path")); // scheme differs
    }
}
```

- [ ] **Step 2: Implement safe_fetch with redirect loop**

```rust
// src/security/ssrf/fetch.rs

use std::collections::HashSet;
use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, COOKIE, LOCATION};
use reqwest::redirect::Policy;
use reqwest::{Client, Method, StatusCode};
use url::Url;

use super::dns::resolve_and_validate;
use super::hostname::{has_url_credentials, is_allowlisted, is_blocked_hostname, is_blocklisted, is_legacy_ip_literal};
use super::ip::is_ip_blocked_by_policy;
use super::policy::SsrfPolicy;
use super::SsrfError;

/// Headers stripped on cross-origin redirects.
const STRIPPED_HEADERS: &[&str] = &["authorization", "cookie", "proxy-authorization"];

/// Request parameters for safe_fetch.
pub struct SafeFetchRequest {
    pub method: Method,
    pub headers: HeaderMap,
    pub body: Option<Vec<u8>>,
    pub timeout: Duration,
}

impl SafeFetchRequest {
    pub fn get(timeout: Duration) -> Self {
        Self {
            method: Method::GET,
            headers: HeaderMap::new(),
            body: None,
            timeout,
        }
    }

    pub fn post(body: Vec<u8>, timeout: Duration) -> Self {
        Self {
            method: Method::POST,
            headers: HeaderMap::new(),
            body: Some(body),
            timeout,
        }
    }

    pub fn with_headers(mut self, headers: HeaderMap) -> Self {
        self.headers = headers;
        self
    }

    pub fn with_method(mut self, method: Method) -> Self {
        self.method = method;
        self
    }
}

/// Response from safe_fetch.
pub struct SafeFetchResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: bytes::Bytes,
    pub final_url: String,
}

/// Validate URL scheme is http or https.
fn validate_scheme(url_str: &str) -> Result<(), SsrfError> {
    if url_str.starts_with("http://") || url_str.starts_with("https://") {
        Ok(())
    } else {
        Err(SsrfError::InvalidUrl(format!(
            "only http:// and https:// schemes are allowed, got: {}",
            url_str.split("://").next().unwrap_or("unknown")
        )))
    }
}

/// Check if two URLs have different origins.
fn is_cross_origin(url_a: &str, url_b: &str) -> bool {
    let parse = |s: &str| -> Option<(String, String, u16)> {
        let u = Url::parse(s).ok()?;
        let scheme = u.scheme().to_string();
        let host = u.host_str()?.to_ascii_lowercase();
        let port = u.port_or_known_default().unwrap_or(0);
        Some((scheme, host, port))
    };
    match (parse(url_a), parse(url_b)) {
        (Some(a), Some(b)) => a != b,
        _ => true, // if we can't parse, assume cross-origin (safe default)
    }
}

/// Perform full SSRF pre-flight validation on a URL.
async fn validate_url_full(
    url_str: &str,
    policy: &SsrfPolicy,
) -> Result<(Url, std::net::SocketAddr), SsrfError> {
    validate_scheme(url_str)?;

    let url = Url::parse(url_str).map_err(|e| SsrfError::InvalidUrl(e.to_string()))?;
    let host = url.host_str().ok_or(SsrfError::NoHost)?;

    // URL credential obfuscation
    if has_url_credentials(url_str) {
        return Err(SsrfError::BlockedAddress(
            "URL contains credentials (possible obfuscation attack)".to_string(),
        ));
    }

    // Legacy IPv4 literal
    if is_legacy_ip_literal(host) {
        return Err(SsrfError::BlockedAddress(format!(
            "non-standard IP literal format: {host}"
        )));
    }

    // Allowlist bypass
    if is_allowlisted(host, &policy.allowed_hosts) {
        let port = url.port_or_known_default().unwrap_or(80);
        let addr = resolve_and_validate(host, port, &SsrfPolicy {
            // For allowlisted hosts, skip IP blocking but still resolve
            allow_private_network: true,
            ..policy.clone()
        }).await?;
        return Ok((url, addr));
    }

    // Hostname blocklist (hardcoded + user-defined)
    if is_blocked_hostname(host) {
        return Err(SsrfError::BlockedAddress(host.to_string()));
    }
    if is_blocklisted(host, &policy.blocked_hosts) {
        return Err(SsrfError::BlockedAddress(host.to_string()));
    }

    // DNS resolve + validate + pin
    let port = url.port_or_known_default().unwrap_or(80);
    let addr = resolve_and_validate(host, port, policy).await?;
    Ok((url, addr))
}

/// Fetch a URL with full SSRF protection: DNS pinning, redirect chain
/// validation, and cross-origin header stripping.
pub async fn safe_fetch(
    url: &str,
    policy: &SsrfPolicy,
    request: SafeFetchRequest,
) -> Result<SafeFetchResponse, SsrfError> {
    // Master switch
    if !policy.enabled {
        return fetch_without_ssrf(url, request).await;
    }

    // Phase 1: validate initial URL + resolve + pin
    let (parsed_url, pinned_addr) = validate_url_full(url, policy).await?;
    let host = parsed_url.host_str().unwrap().to_string();

    // Build client with DNS pinning and no auto-redirect
    let client = Client::builder()
        .redirect(Policy::none())
        .timeout(request.timeout)
        .resolve(&host, pinned_addr)
        .build()
        .map_err(|e| SsrfError::FetchFailed(e.to_string()))?;

    // Build request
    let mut req_builder = client.request(request.method.clone(), parsed_url.as_str());
    req_builder = req_builder.headers(request.headers.clone());
    if let Some(body) = &request.body {
        req_builder = req_builder.body(body.clone());
    }

    let mut current_url = url.to_string();
    let mut current_headers = request.headers.clone();
    let mut visited = HashSet::new();
    visited.insert(current_url.clone());
    let mut redirect_count: u8 = 0;

    // Send initial request
    let mut response = req_builder
        .send()
        .await
        .map_err(|e| SsrfError::FetchFailed(e.to_string()))?;

    // Phase 2: redirect loop
    while response.status().is_redirection() {
        redirect_count += 1;
        if redirect_count > policy.max_redirects {
            return Err(SsrfError::TooManyRedirects(policy.max_redirects));
        }

        // Extract Location header
        let location = response
            .headers()
            .get(LOCATION)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| SsrfError::FetchFailed("redirect without Location header".to_string()))?;

        // Resolve relative URLs against current URL
        let next_url = Url::parse(location)
            .or_else(|_| Url::parse(&current_url).and_then(|base| base.join(location)))
            .map_err(|e| SsrfError::InvalidUrl(format!("invalid redirect URL: {e}")))?
            .to_string();

        // Loop detection
        if !visited.insert(next_url.clone()) {
            return Err(SsrfError::FetchFailed("redirect loop detected".to_string()));
        }

        // Validate redirect target
        let (next_parsed, next_addr) = validate_url_full(&next_url, policy).await?;
        let next_host = next_parsed.host_str().unwrap().to_string();

        // Cross-origin header stripping
        if policy.strip_auth_on_cross_origin && is_cross_origin(&current_url, &next_url) {
            for header_name in STRIPPED_HEADERS {
                current_headers.remove(*header_name);
            }
        }

        // Build new client with pinned DNS for redirect target
        let redirect_client = Client::builder()
            .redirect(Policy::none())
            .timeout(request.timeout)
            .resolve(&next_host, next_addr)
            .build()
            .map_err(|e| SsrfError::FetchFailed(e.to_string()))?;

        // Follow redirect with GET (POST → GET on 301/302/303)
        let method = if matches!(
            response.status(),
            StatusCode::MOVED_PERMANENTLY | StatusCode::FOUND | StatusCode::SEE_OTHER
        ) {
            Method::GET
        } else {
            request.method.clone()
        };

        response = redirect_client
            .request(method, next_parsed.as_str())
            .headers(current_headers.clone())
            .send()
            .await
            .map_err(|e| SsrfError::FetchFailed(e.to_string()))?;

        current_url = next_url;
    }

    // Collect response
    let status = response.status();
    let headers = response.headers().clone();
    let body = response
        .bytes()
        .await
        .map_err(|e| SsrfError::FetchFailed(e.to_string()))?;

    Ok(SafeFetchResponse {
        status,
        headers,
        body,
        final_url: current_url,
    })
}

/// Bypass fetch when SSRF is disabled (master switch off).
async fn fetch_without_ssrf(
    url: &str,
    request: SafeFetchRequest,
) -> Result<SafeFetchResponse, SsrfError> {
    let client = Client::builder()
        .timeout(request.timeout)
        .build()
        .map_err(|e| SsrfError::FetchFailed(e.to_string()))?;

    let mut req_builder = client.request(request.method, url);
    req_builder = req_builder.headers(request.headers);
    if let Some(body) = request.body {
        req_builder = req_builder.body(body);
    }

    let response = req_builder
        .send()
        .await
        .map_err(|e| SsrfError::FetchFailed(e.to_string()))?;

    let final_url = response.url().to_string();
    let status = response.status();
    let headers = response.headers().clone();
    let body = response
        .bytes()
        .await
        .map_err(|e| SsrfError::FetchFailed(e.to_string()))?;

    Ok(SafeFetchResponse {
        status,
        headers,
        body,
        final_url,
    })
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p alephcore --lib ssrf::fetch -- --nocapture`
Expected: All tests PASS

- [ ] **Step 4: Commit**

```bash
git add src/security/ssrf/fetch.rs
git commit -m "security: add safe_fetch with redirect chain validation and DNS pinning"
```

---

## Task 6: Create mod.rs — Wire up the module + backward-compat public API

**Files:**
- Create: `src/security/ssrf/mod.rs`
- Delete: `src/security/ssrf.rs`
- Modify: `src/security/mod.rs`

- [ ] **Step 1: Create ssrf directory and move old file**

```bash
cd /Volumes/TBU/Workspace/Aleph
mkdir -p src/security/ssrf
# Old ssrf.rs becomes ssrf/mod.rs temporarily during transition
mv src/security/ssrf.rs src/security/ssrf_old.rs
```

- [ ] **Step 2: Write mod.rs with public API**

```rust
// src/security/ssrf/mod.rs

//! Unified SSRF protection engine.
//!
//! Validates URLs before outbound HTTP requests to prevent Server-Side Request
//! Forgery attacks. Blocks private networks, loopback addresses, cloud metadata
//! endpoints, legacy IP literals, and performs DNS pinning to prevent rebinding.

pub mod policy;
pub mod ip;
pub mod hostname;
pub mod dns;
pub mod fetch;

pub use policy::SsrfPolicy;
pub use fetch::{safe_fetch, SafeFetchRequest, SafeFetchResponse};

use thiserror::Error;
use url::Url;

/// Errors returned by SSRF validation.
#[derive(Debug, Error)]
pub enum SsrfError {
    #[error("invalid URL: {0}")]
    InvalidUrl(String),

    #[error("blocked address: {0}")]
    BlockedAddress(String),

    #[error("DNS resolution failed for {host}: {reason}")]
    DnsResolutionFailed { host: String, reason: String },

    #[error("URL has no host")]
    NoHost,

    #[error("too many redirects (limit: {0})")]
    TooManyRedirects(u8),

    #[error("fetch failed: {0}")]
    FetchFailed(String),
}

// --- Backward-compatible public API ---

/// Validates a URL synchronously (no DNS resolution).
/// Checks scheme, host blocklist, IP literal ranges, legacy IP formats,
/// and URL credential obfuscation.
pub fn validate_url(url_str: &str, policy: &SsrfPolicy) -> Result<Url, SsrfError> {
    if !policy.enabled {
        return Url::parse(url_str).map_err(|e| SsrfError::InvalidUrl(e.to_string()));
    }

    let url = Url::parse(url_str).map_err(|e| SsrfError::InvalidUrl(e.to_string()))?;
    let host = url.host_str().ok_or(SsrfError::NoHost)?;

    // URL credential obfuscation
    if hostname::has_url_credentials(url_str) {
        return Err(SsrfError::BlockedAddress(
            "URL contains credentials (possible obfuscation attack)".to_string(),
        ));
    }

    // Legacy IPv4 literal
    if hostname::is_legacy_ip_literal(host) {
        return Err(SsrfError::BlockedAddress(format!(
            "non-standard IP literal format: {host}"
        )));
    }

    // Allowlist bypass
    if hostname::is_allowlisted(host, &policy.allowed_hosts) {
        return Ok(url);
    }

    // Hostname blocklist (hardcoded + user-defined)
    if hostname::is_blocked_hostname(host) {
        return Err(SsrfError::BlockedAddress(host.to_string()));
    }
    if hostname::is_blocklisted(host, &policy.blocked_hosts) {
        return Err(SsrfError::BlockedAddress(host.to_string()));
    }

    // IP literal check
    let ip_from_url = match url.host() {
        Some(url::Host::Ipv4(v4)) => Some(std::net::IpAddr::V4(v4)),
        Some(url::Host::Ipv6(v6)) => Some(std::net::IpAddr::V6(v6)),
        _ => None,
    };
    if let Some(addr) = ip_from_url {
        if ip::is_ip_blocked_by_policy(addr, policy) {
            return Err(SsrfError::BlockedAddress(addr.to_string()));
        }
    }

    Ok(url)
}

/// Validates a URL asynchronously, including DNS rebinding defense.
pub async fn validate_url_async(url_str: &str, policy: &SsrfPolicy) -> Result<Url, SsrfError> {
    // Run all sync checks first
    let url = validate_url(url_str, policy)?;
    let host = url.host_str().ok_or(SsrfError::NoHost)?;

    // If allowlisted, skip DNS validation
    if hostname::is_allowlisted(host, &policy.allowed_hosts) {
        return Ok(url);
    }

    // If host is a literal IP, already validated in sync check
    match url.host() {
        Some(url::Host::Ipv4(_)) | Some(url::Host::Ipv6(_)) => return Ok(url),
        _ => {}
    }

    // DNS resolution — check all returned IPs
    let port = url.port_or_known_default().unwrap_or(80);
    let _pinned = dns::resolve_and_validate(host, port, policy).await?;

    Ok(url)
}
```

- [ ] **Step 3: Remove old ssrf_old.rs**

```bash
rm src/security/ssrf_old.rs
```

- [ ] **Step 4: Verify security/mod.rs declaration is correct**

The existing `pub mod ssrf;` in `src/security/mod.rs` will automatically resolve to `ssrf/mod.rs` — no change needed.

- [ ] **Step 5: Run all existing SSRF tests to verify backward compatibility**

Run: `cargo test -p alephcore --lib ssrf -- --nocapture`
Expected: All existing tests PASS (they import `crate::security::ssrf::{validate_url, SsrfPolicy}` which still exist)

- [ ] **Step 6: Run full build**

Run: `cargo check -p alephcore`
Expected: No errors

- [ ] **Step 7: Commit**

```bash
git add src/security/ssrf/ src/security/mod.rs
git rm src/security/ssrf_old.rs 2>/dev/null; true
git commit -m "security: convert ssrf.rs to ssrf/ module with unified engine"
```

---

## Task 7: Migrate web_fetch.rs to safe_fetch

**Files:**
- Modify: `src/builtin_tools/web_fetch.rs`

- [ ] **Step 1: Replace validate_url + client.get with safe_fetch**

In `src/builtin_tools/web_fetch.rs`, replace the `call_impl` method body. Key changes:
- Remove `use crate::security::ssrf::{validate_url, SsrfPolicy};`
- Add `use crate::security::ssrf::{safe_fetch, SafeFetchRequest, SsrfPolicy};`
- Remove the manual scheme check (`starts_with("http://")`)
- Remove the `validate_url()` call
- Remove the `self.client.get()` call
- Replace with a single `safe_fetch()` call
- Remove `client: Client` from struct (no longer needed for fetching)

Replace the fetch section (lines ~119-148) with:

```rust
        // SSRF-protected fetch
        let ssrf_policy = SsrfPolicy::default();
        let fetch_request = SafeFetchRequest::get(
            std::time::Duration::from_secs(self.timeout_secs),
        );

        let fetch_response = safe_fetch(&args.url, &ssrf_policy, fetch_request)
            .await
            .map_err(|e| {
                let error_msg = format!("Fetch blocked or failed: {}", e);
                notify_tool_result(Self::NAME, &error_msg, false);
                ToolError::Network(error_msg)
            })?;

        if !fetch_response.status.is_success() {
            let error_msg = format!("HTTP error: {} for URL: {}", fetch_response.status, args.url);
            notify_tool_result(Self::NAME, &error_msg, false);
            return Err(ToolError::Network(error_msg));
        }

        let bytes = fetch_response.body;
```

- [ ] **Step 2: Simplify WebFetchTool struct — remove client field**

The `client` field is no longer needed since `safe_fetch` manages its own client. Remove it from the struct, `new()`, `with_policy()`, and `Clone` impl.

- [ ] **Step 3: Run existing web_fetch tests**

Run: `cargo test -p alephcore --lib web_fetch -- --nocapture`
Expected: All existing tests PASS

- [ ] **Step 4: Run full build**

Run: `cargo check -p alephcore`
Expected: No errors

- [ ] **Step 5: Commit**

```bash
git add src/builtin_tools/web_fetch.rs
git commit -m "security: migrate web_fetch to safe_fetch with DNS pinning"
```

---

## Task 8: Add SSRF to webhook_target.rs

**Files:**
- Modify: `src/tasks/cron/webhook_target.rs`

- [ ] **Step 1: Add safe_fetch to webhook delivery**

Replace the raw `self.client.post(url)` / `self.client.put(url)` with `safe_fetch`:

```rust
// src/tasks/cron/webhook_target.rs

use async_trait::async_trait;
use reqwest::header::{HeaderMap, HeaderValue};
use reqwest::Method;

use crate::security::ssrf::{safe_fetch, SafeFetchRequest, SsrfPolicy};
use crate::tasks::shared::delivery::{
    DeliveryError, DeliveryOutcome, DeliveryPayload, DeliveryTarget, DeliveryTargetConfig,
};

pub struct WebhookTarget;

impl Default for WebhookTarget {
    fn default() -> Self {
        Self::new()
    }
}

impl WebhookTarget {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl DeliveryTarget for WebhookTarget {
    fn kind(&self) -> &str {
        "webhook"
    }

    async fn deliver(
        &self,
        payload: &DeliveryPayload,
        config: &DeliveryTargetConfig,
    ) -> Result<DeliveryOutcome, DeliveryError> {
        let (url, method, headers) = match config {
            DeliveryTargetConfig::Webhook {
                url,
                method,
                headers,
            } => (url, method, headers),
            _ => return Err(DeliveryError::InvalidConfig("Expected Webhook config".into())),
        };

        let body = serde_json::json!({
            "source_type": payload.source_type,
            "task_name": payload.task_name,
            "agent_id": payload.agent_id,
            "output": payload.output,
            "channel_id": payload.channel_id,
            "metadata": payload.metadata,
        });

        let body_bytes = serde_json::to_vec(&body)
            .map_err(|e| DeliveryError::Failed(format!("JSON serialize failed: {e}")))?;

        let method_str = method.as_deref().unwrap_or("POST");
        let req_method = match method_str {
            "PUT" => Method::PUT,
            _ => Method::POST,
        };

        let mut header_map = HeaderMap::new();
        header_map.insert("content-type", HeaderValue::from_static("application/json"));
        if let Some(hdrs) = headers {
            for (key, value) in hdrs {
                if let (Ok(name), Ok(val)) = (
                    reqwest::header::HeaderName::from_bytes(key.as_bytes()),
                    HeaderValue::from_str(value),
                ) {
                    header_map.insert(name, val);
                }
            }
        }

        let ssrf_policy = SsrfPolicy::default();
        let fetch_request = SafeFetchRequest::post(body_bytes, std::time::Duration::from_secs(30))
            .with_method(req_method)
            .with_headers(header_map);

        match safe_fetch(url, &ssrf_policy, fetch_request).await {
            Ok(resp) if resp.status.is_success() => Ok(DeliveryOutcome {
                target_kind: "webhook".to_string(),
                success: true,
                message: Some(format!("HTTP {}", resp.status)),
            }),
            Ok(resp) => Err(DeliveryError::Failed(format!(
                "HTTP {} from {}",
                resp.status, url
            ))),
            Err(e) => Err(DeliveryError::Failed(format!("SSRF/fetch error: {}", e))),
        }
    }
}
```

- [ ] **Step 2: Run build**

Run: `cargo check -p alephcore`
Expected: No errors

- [ ] **Step 3: Commit**

```bash
git add src/tasks/cron/webhook_target.rs
git commit -m "security: add SSRF protection to webhook delivery via safe_fetch"
```

---

## Task 9: Migrate media_download.rs to safe_fetch

**Files:**
- Modify: `src/gateway/pipeline/media_download.rs`

- [ ] **Step 1: Replace validate_url + client.get with safe_fetch**

Two places to change: `download_attachment_url()` and `download_url()`.

In both methods, replace:
```rust
let ssrf_policy = SsrfPolicy::default();
validate_url(url, &ssrf_policy).map_err(|e| format!("SSRF blocked: {e}"))?;
let response = self.http_client.get(url).send().await...
```

With:
```rust
use crate::security::ssrf::{safe_fetch, SafeFetchRequest, SsrfPolicy};

let ssrf_policy = SsrfPolicy::default();
let fetch_request = SafeFetchRequest::get(std::time::Duration::from_secs(30));
let response = safe_fetch(url, &ssrf_policy, fetch_request)
    .await
    .map_err(|e| format!("SSRF/fetch error: {e}"))?;
let bytes = response.body;
```

Remove `http_client: reqwest::Client` from `MediaDownloader` struct since `safe_fetch` handles its own client.

Update imports: remove `validate_url`, add `safe_fetch, SafeFetchRequest`.

- [ ] **Step 2: Run existing media_download tests**

Run: `cargo test -p alephcore --lib media_download -- --nocapture`
Expected: All existing tests PASS

- [ ] **Step 3: Commit**

```bash
git add src/gateway/pipeline/media_download.rs
git commit -m "security: migrate media_download to safe_fetch"
```

---

## Task 10: Update mcp/transport/http.rs import path

**Files:**
- Modify: `src/mcp/transport/http.rs`

- [ ] **Step 1: Update import**

The import `use crate::security::ssrf::{validate_url, SsrfPolicy};` should still work because the public API is preserved. Verify build compiles.

Run: `cargo check -p alephcore`
Expected: No errors — the re-exported `validate_url` and `SsrfPolicy` match the old signatures.

- [ ] **Step 2: Commit (only if any change was needed)**

```bash
git add src/mcp/transport/http.rs
git commit -m "security: verify mcp transport ssrf import compatibility"
```

---

## Task 11: Refactor browser/network_policy.rs to thin wrapper

**Files:**
- Modify: `src/browser/network_policy.rs`
- Modify: `src/browser/manager.rs`

- [ ] **Step 1: Replace internal logic with core engine calls**

```rust
// src/browser/network_policy.rs

//! Browser-level SSRF guard — thin wrapper over the core SSRF engine.

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::security::ssrf::{self, SsrfPolicy};

/// Configuration for browser SSRF protection policy.
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
}

fn default_true() -> bool {
    true
}

impl Default for SsrfConfig {
    fn default() -> Self {
        Self {
            block_private: true,
            blocked_domains: Vec::new(),
            allowed_domains: Vec::new(),
        }
    }
}

/// Reasons a URL can be rejected by the browser SSRF policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyViolation {
    PrivateNetwork(String),
    BlockedDomain(String),
    NotInAllowlist(String),
    InvalidUrl(String),
}

impl fmt::Display for PolicyViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PolicyViolation::PrivateNetwork(host) => {
                write!(f, "blocked: host '{host}' resolves to a private network")
            }
            PolicyViolation::BlockedDomain(domain) => {
                write!(f, "blocked: domain '{domain}' matches a block pattern")
            }
            PolicyViolation::NotInAllowlist(domain) => {
                write!(f, "blocked: domain '{domain}' is not in the allowlist")
            }
            PolicyViolation::InvalidUrl(reason) => {
                write!(f, "invalid URL: {reason}")
            }
        }
    }
}

impl std::error::Error for PolicyViolation {}

impl From<ssrf::SsrfError> for PolicyViolation {
    fn from(err: ssrf::SsrfError) -> Self {
        match err {
            ssrf::SsrfError::InvalidUrl(msg) => PolicyViolation::InvalidUrl(msg),
            ssrf::SsrfError::NoHost => PolicyViolation::InvalidUrl("no host in URL".to_string()),
            ssrf::SsrfError::BlockedAddress(addr) => PolicyViolation::PrivateNetwork(addr),
            ssrf::SsrfError::DnsResolutionFailed { host, reason } => {
                PolicyViolation::InvalidUrl(format!("DNS failed for {host}: {reason}"))
            }
            ssrf::SsrfError::TooManyRedirects(n) => {
                PolicyViolation::InvalidUrl(format!("too many redirects ({n})"))
            }
            ssrf::SsrfError::FetchFailed(msg) => PolicyViolation::InvalidUrl(msg),
        }
    }
}

/// Browser SSRF guard — delegates to the core SSRF engine.
#[derive(Debug, Clone, Default)]
pub struct BrowserSsrfGuard {
    config: SsrfConfig,
}

impl BrowserSsrfGuard {
    pub fn new(config: SsrfConfig) -> Self {
        Self { config }
    }

    /// Validate a URL against the browser SSRF policy.
    pub fn check_url(&self, url_str: &str) -> Result<(), PolicyViolation> {
        let core_policy = self.to_ssrf_policy();

        // Core engine validation (sync — IP literal + hostname checks)
        ssrf::validate_url(url_str, &core_policy)?;

        // Additional browser-specific: allowlist mode
        if !self.config.allowed_domains.is_empty() {
            let url = url::Url::parse(url_str)
                .map_err(|e| PolicyViolation::InvalidUrl(e.to_string()))?;
            let host = url
                .host_str()
                .ok_or_else(|| PolicyViolation::InvalidUrl("no host".to_string()))?;

            let matched = self
                .config
                .allowed_domains
                .iter()
                .any(|pat| ssrf::hostname::is_allowlisted(host, &[pat.clone()]));
            if !matched {
                return Err(PolicyViolation::NotInAllowlist(host.to_string()));
            }
        }

        Ok(())
    }

    /// Convert browser config to core SsrfPolicy.
    fn to_ssrf_policy(&self) -> SsrfPolicy {
        SsrfPolicy {
            enabled: true,
            allow_private_network: !self.config.block_private,
            allowed_hosts: self.config.allowed_domains.clone(),
            blocked_hosts: self.config.blocked_domains.clone(),
            ..SsrfPolicy::default()
        }
    }
}
```

- [ ] **Step 2: Update browser/manager.rs imports**

Change:
```rust
use super::network_policy::{PolicyViolation, SsrfPolicy};
```
To:
```rust
use super::network_policy::{PolicyViolation, BrowserSsrfGuard};
```

Update the `ssrf_policy` field type from `SsrfPolicy` to `BrowserSsrfGuard` and the constructor accordingly.

- [ ] **Step 3: Run browser tests**

Run: `cargo test -p alephcore --lib browser -- --nocapture`
Expected: All tests PASS

- [ ] **Step 4: Run full build**

Run: `cargo check -p alephcore`
Expected: No errors

- [ ] **Step 5: Commit**

```bash
git add src/browser/network_policy.rs src/browser/manager.rs
git commit -m "security: refactor browser SSRF to thin wrapper over core engine"
```

---

## Task 12: Add SSRF config to security_config handler

**Files:**
- Modify: `src/gateway/handlers/security_config.rs`

- [ ] **Step 1: Extend SecurityConfig struct**

Add SSRF fields to the server-side `SecurityConfig`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityConfig {
    pub require_auth: bool,
    pub enable_pairing: bool,
    pub allow_guest: bool,
    #[serde(default = "default_network_access")]
    pub network_access: NetworkAccess,
    // SSRF outbound protection settings
    #[serde(default = "default_true")]
    pub ssrf_enabled: bool,
    #[serde(default)]
    pub ssrf_allow_tool_private_network: bool,
    #[serde(default)]
    pub ssrf_allow_webhook_private_network: bool,
    #[serde(default = "default_max_redirects")]
    pub ssrf_max_redirects: u8,
    #[serde(default)]
    pub ssrf_allowed_hosts: Vec<String>,
    #[serde(default)]
    pub ssrf_blocked_hosts: Vec<String>,
}

fn default_true() -> bool { true }
fn default_max_redirects() -> u8 { 5 }
```

- [ ] **Step 2: Add read/write for [security.ssrf] in config TOML**

Add a `read_ssrf_config_from_toml` function and a `write_ssrf_config_to_toml` function following the same pattern as `read_gateway_host_from_config` / `write_gateway_host_to_config`.

- [ ] **Step 3: Wire into handle_get and handle_update**

In `handle_get`, read SSRF config from TOML and populate the response.
In `handle_update`, write SSRF settings to `[security.ssrf]` section.

- [ ] **Step 4: Run build**

Run: `cargo check -p alephcore`
Expected: No errors

- [ ] **Step 5: Commit**

```bash
git add src/gateway/handlers/security_config.rs
git commit -m "security: add SSRF config read/write to security_config handler"
```

---

## Task 13: Add Panel UI — OutboundSecuritySection

**Files:**
- Modify: `interfaces/webchat/src/api/security.rs`
- Modify: `interfaces/webchat/src/views/settings/security.rs`

- [ ] **Step 1: Extend Panel SecurityConfig API type**

In `interfaces/webchat/src/api/security.rs`, add SSRF fields:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityConfig {
    pub require_auth: bool,
    pub enable_pairing: bool,
    pub allow_guest: bool,
    #[serde(default = "default_network_access")]
    pub network_access: String,
    // SSRF outbound protection
    #[serde(default = "default_true")]
    pub ssrf_enabled: bool,
    #[serde(default)]
    pub ssrf_allow_tool_private_network: bool,
    #[serde(default)]
    pub ssrf_allow_webhook_private_network: bool,
    #[serde(default = "default_max_redirects")]
    pub ssrf_max_redirects: u8,
    #[serde(default)]
    pub ssrf_allowed_hosts: Vec<String>,
    #[serde(default)]
    pub ssrf_blocked_hosts: Vec<String>,
}

fn default_true() -> bool { true }
fn default_max_redirects() -> u8 { 5 }
```

- [ ] **Step 2: Add OutboundSecuritySection component**

In `interfaces/webchat/src/views/settings/security.rs`, add:

```rust
#[component]
fn OutboundSecuritySection(
    config: RwSignal<Option<SecurityConfig>>,
) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <div class="bg-surface-raised rounded-lg border border-border p-6">
            <h2 class="text-lg font-semibold text-text-primary mb-4">
                {t!(i18n, settings.security.outbound_protection)}
            </h2>
            <p class="text-sm text-text-tertiary mb-4">
                {t!(i18n, settings.security.outbound_protection_desc)}
            </p>

            <div class="space-y-4">
                // Master toggle
                <label class="flex items-center space-x-3 cursor-pointer">
                    <input
                        type="checkbox"
                        checked=move || config.get().map(|c| c.ssrf_enabled).unwrap_or(true)
                        on:change=move |ev| {
                            if let Some(mut cfg) = config.get() {
                                cfg.ssrf_enabled = event_target_checked(&ev);
                                config.set(Some(cfg));
                            }
                        }
                        class="w-4 h-4 text-primary focus:ring-primary/30 rounded"
                    />
                    <div>
                        <div class="font-medium text-text-primary">{t!(i18n, settings.security.ssrf_enabled)}</div>
                        <div class="text-xs text-text-tertiary">{t!(i18n, settings.security.ssrf_enabled_desc)}</div>
                    </div>
                </label>

                // Tool LAN access
                <label class="flex items-center space-x-3 cursor-pointer ml-4">
                    <input
                        type="checkbox"
                        checked=move || config.get().map(|c| c.ssrf_allow_tool_private_network).unwrap_or(false)
                        on:change=move |ev| {
                            if let Some(mut cfg) = config.get() {
                                cfg.ssrf_allow_tool_private_network = event_target_checked(&ev);
                                config.set(Some(cfg));
                            }
                        }
                        disabled=move || !config.get().map(|c| c.ssrf_enabled).unwrap_or(true)
                        class="w-4 h-4 text-primary focus:ring-primary/30 rounded disabled:opacity-50"
                    />
                    <div>
                        <div class="font-medium text-text-primary">{t!(i18n, settings.security.ssrf_allow_tool_lan)}</div>
                        <div class="text-xs text-text-tertiary">{t!(i18n, settings.security.ssrf_allow_tool_lan_desc)}</div>
                    </div>
                </label>

                // Webhook LAN access
                <label class="flex items-center space-x-3 cursor-pointer ml-4">
                    <input
                        type="checkbox"
                        checked=move || config.get().map(|c| c.ssrf_allow_webhook_private_network).unwrap_or(false)
                        on:change=move |ev| {
                            if let Some(mut cfg) = config.get() {
                                cfg.ssrf_allow_webhook_private_network = event_target_checked(&ev);
                                config.set(Some(cfg));
                            }
                        }
                        disabled=move || !config.get().map(|c| c.ssrf_enabled).unwrap_or(true)
                        class="w-4 h-4 text-primary focus:ring-primary/30 rounded disabled:opacity-50"
                    />
                    <div>
                        <div class="font-medium text-text-primary">{t!(i18n, settings.security.ssrf_allow_webhook_lan)}</div>
                        <div class="text-xs text-text-tertiary">{t!(i18n, settings.security.ssrf_allow_webhook_lan_desc)}</div>
                    </div>
                </label>

                // Max redirects
                <div class="ml-4">
                    <label class="block text-sm font-medium text-text-secondary mb-1">
                        {t!(i18n, settings.security.ssrf_max_redirects)}
                    </label>
                    <input
                        type="number"
                        min="0"
                        max="20"
                        prop:value=move || config.get().map(|c| c.ssrf_max_redirects.to_string()).unwrap_or_else(|| "5".to_string())
                        on:change=move |ev| {
                            if let Some(mut cfg) = config.get() {
                                if let Ok(v) = event_target_value(&ev).parse::<u8>() {
                                    cfg.ssrf_max_redirects = v.min(20);
                                    config.set(Some(cfg));
                                }
                            }
                        }
                        disabled=move || !config.get().map(|c| c.ssrf_enabled).unwrap_or(true)
                        class="w-24 px-3 py-1 bg-surface-sunken border border-border rounded text-text-primary disabled:opacity-50"
                    />
                    <p class="text-xs text-text-tertiary mt-1">{t!(i18n, settings.security.ssrf_max_redirects_desc)}</p>
                </div>

                // Allowed hosts (simple textarea for MVP)
                <div class="ml-4">
                    <label class="block text-sm font-medium text-text-secondary mb-1">
                        {t!(i18n, settings.security.ssrf_allowed_hosts)}
                    </label>
                    <textarea
                        prop:value=move || config.get().map(|c| c.ssrf_allowed_hosts.join("\n")).unwrap_or_default()
                        on:change=move |ev| {
                            if let Some(mut cfg) = config.get() {
                                cfg.ssrf_allowed_hosts = event_target_value(&ev)
                                    .lines()
                                    .map(|l| l.trim().to_string())
                                    .filter(|l| !l.is_empty())
                                    .collect();
                                config.set(Some(cfg));
                            }
                        }
                        disabled=move || !config.get().map(|c| c.ssrf_enabled).unwrap_or(true)
                        placeholder=move || t_string!(i18n, settings.security.ssrf_allowed_hosts_placeholder).to_string()
                        rows="3"
                        class="w-full px-3 py-2 bg-surface-sunken border border-border rounded text-text-primary text-sm disabled:opacity-50"
                    />
                    <p class="text-xs text-text-tertiary mt-1">{t!(i18n, settings.security.ssrf_allowed_hosts_desc)}</p>
                </div>

                // Blocked hosts (simple textarea for MVP)
                <div class="ml-4">
                    <label class="block text-sm font-medium text-text-secondary mb-1">
                        {t!(i18n, settings.security.ssrf_blocked_hosts)}
                    </label>
                    <textarea
                        prop:value=move || config.get().map(|c| c.ssrf_blocked_hosts.join("\n")).unwrap_or_default()
                        on:change=move |ev| {
                            if let Some(mut cfg) = config.get() {
                                cfg.ssrf_blocked_hosts = event_target_value(&ev)
                                    .lines()
                                    .map(|l| l.trim().to_string())
                                    .filter(|l| !l.is_empty())
                                    .collect();
                                config.set(Some(cfg));
                            }
                        }
                        disabled=move || !config.get().map(|c| c.ssrf_enabled).unwrap_or(true)
                        placeholder=move || t_string!(i18n, settings.security.ssrf_blocked_hosts_placeholder).to_string()
                        rows="3"
                        class="w-full px-3 py-2 bg-surface-sunken border border-border rounded text-text-primary text-sm disabled:opacity-50"
                    />
                    <p class="text-xs text-text-tertiary mt-1">{t!(i18n, settings.security.ssrf_blocked_hosts_desc)}</p>
                </div>
            </div>
        </div>
    }
}
```

- [ ] **Step 3: Insert OutboundSecuritySection into SecurityView**

Between `<NetworkAccessSection>` and `<PIISection>`:

```rust
<NetworkAccessSection config=config />
<OutboundSecuritySection config=config />  // NEW
<PIISection config=search_config />
```

- [ ] **Step 4: Add i18n keys**

Add keys to the appropriate locale files (en.json and zh.json) for all `settings.security.ssrf_*` and `settings.security.outbound_protection*` keys.

- [ ] **Step 5: Build WASM**

Run: `cd interfaces/webchat && trunk build`
Expected: No errors

- [ ] **Step 6: Commit**

```bash
git add interfaces/webchat/src/api/security.rs interfaces/webchat/src/views/settings/security.rs
git commit -m "security: add outbound protection settings to Panel UI"
```

---

## Task 14: Add i18n keys for SSRF settings

**Files:**
- Modify: i18n locale files (en.json and zh.json)

- [ ] **Step 1: Find locale files**

Run: `find interfaces/webchat/src -name "*.json" -path "*locale*" -o -name "*.json" -path "*i18n*" | head -10`

- [ ] **Step 2: Add English keys**

```json
{
    "settings.security.outbound_protection": "Outbound Request Protection",
    "settings.security.outbound_protection_desc": "Control how Aleph validates URLs before making outbound HTTP requests. These settings protect against Server-Side Request Forgery (SSRF) attacks.",
    "settings.security.ssrf_enabled": "Enable SSRF Protection",
    "settings.security.ssrf_enabled_desc": "Master switch for outbound request validation. Disabling this removes all SSRF protections (dangerous).",
    "settings.security.ssrf_allow_tool_lan": "Allow Tools to Access LAN",
    "settings.security.ssrf_allow_tool_lan_desc": "Allow AI tools (web_fetch, etc.) to access private network addresses (10.x, 172.16.x, 192.168.x).",
    "settings.security.ssrf_allow_webhook_lan": "Allow Webhooks to Access LAN",
    "settings.security.ssrf_allow_webhook_lan_desc": "Allow cron webhook delivery to private network addresses.",
    "settings.security.ssrf_max_redirects": "Max Redirects",
    "settings.security.ssrf_max_redirects_desc": "Maximum number of HTTP redirects to follow (0-20).",
    "settings.security.ssrf_allowed_hosts": "Trusted Hosts",
    "settings.security.ssrf_allowed_hosts_desc": "One host per line. Supports wildcards: *.corp.internal",
    "settings.security.ssrf_allowed_hosts_placeholder": "*.corp.internal\nnas.local",
    "settings.security.ssrf_blocked_hosts": "Blocked Hosts",
    "settings.security.ssrf_blocked_hosts_desc": "One host per line. Supports wildcards: *.malware.com",
    "settings.security.ssrf_blocked_hosts_placeholder": "*.malware.com\nevil.org"
}
```

- [ ] **Step 3: Add Chinese keys**

```json
{
    "settings.security.outbound_protection": "出站请求防护",
    "settings.security.outbound_protection_desc": "控制 Aleph 在发起 HTTP 出站请求前如何验证 URL。这些设置可防御服务端请求伪造 (SSRF) 攻击。",
    "settings.security.ssrf_enabled": "启用 SSRF 防护",
    "settings.security.ssrf_enabled_desc": "出站请求验证的总开关。关闭将移除所有 SSRF 防护（危险）。",
    "settings.security.ssrf_allow_tool_lan": "允许工具访问内网",
    "settings.security.ssrf_allow_tool_lan_desc": "允许 AI 工具（web_fetch 等）访问私有网络地址（10.x、172.16.x、192.168.x）。",
    "settings.security.ssrf_allow_webhook_lan": "允许 Webhook 访问内网",
    "settings.security.ssrf_allow_webhook_lan_desc": "允许定时任务 Webhook 投递到私有网络地址。",
    "settings.security.ssrf_max_redirects": "最大重定向次数",
    "settings.security.ssrf_max_redirects_desc": "出站请求跟随 HTTP 重定向的最大跳数（0-20）。",
    "settings.security.ssrf_allowed_hosts": "信任主机",
    "settings.security.ssrf_allowed_hosts_desc": "每行一个主机。支持通配符：*.corp.internal",
    "settings.security.ssrf_allowed_hosts_placeholder": "*.corp.internal\nnas.local",
    "settings.security.ssrf_blocked_hosts": "阻断主机",
    "settings.security.ssrf_blocked_hosts_desc": "每行一个主机。支持通配符：*.malware.com",
    "settings.security.ssrf_blocked_hosts_placeholder": "*.malware.com\nevil.org"
}
```

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/i18n/
git commit -m "i18n: add SSRF outbound protection setting keys (en/zh)"
```

---

## Task 15: Final build + full test suite

**Files:** None (verification only)

- [ ] **Step 1: Run full Rust build**

Run: `cargo build -p alephcore`
Expected: No errors

- [ ] **Step 2: Run all SSRF tests**

Run: `cargo test -p alephcore --lib ssrf -- --nocapture`
Expected: All tests PASS

- [ ] **Step 3: Run all core tests**

Run: `cargo test -p alephcore --lib -- --nocapture`
Expected: All tests PASS, no regressions

- [ ] **Step 4: Run clippy**

Run: `cargo clippy -p alephcore -- -D warnings`
Expected: No warnings

- [ ] **Step 5: Build WASM Panel**

Run: `cd interfaces/webchat && trunk build`
Expected: No errors

- [ ] **Step 6: Final commit**

```bash
git add -A
git commit -m "security: SSRF engine upgrade - unified engine, DNS pinning, redirect validation, Panel UI"
```
