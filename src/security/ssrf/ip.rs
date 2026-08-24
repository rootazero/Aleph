//! IP address classification for SSRF protection.
//!
//! Validates IPv4 and IPv6 addresses against private, loopback, link-local,
//! multicast, reserved, and other non-routable ranges. Handles IPv6 transition
//! mechanisms that embed IPv4 addresses (mapped, NAT64, 6to4, Teredo, compatible).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use super::policy::SsrfPolicy;

/// Returns true if the IPv4 address falls in any blocked range.
///
/// Blocked ranges: 0.0.0.0/8, 10.0.0.0/8, 100.64.0.0/10 (CGNAT), 127.0.0.0/8,
/// 169.254.0.0/16, 172.16.0.0/12, 192.0.2.0/24 (TEST-NET-1), 192.168.0.0/16,
/// 198.18.0.0/15 (benchmark), 198.51.100.0/24 (TEST-NET-2), 203.0.113.0/24 (TEST-NET-3),
/// 224.0.0.0/4 (multicast), 240.0.0.0/4 (reserved + broadcast).
pub(crate) const fn is_blocked_ipv4(ip: Ipv4Addr) -> bool {
    let octets = ip.octets();

    is_private_or_special_ipv4(octets)
        || is_cgnat_or_test_ipv4(octets)
        || is_multicast_or_reserved_ipv4(octets)
}

const fn is_private_or_special_ipv4(octets: [u8; 4]) -> bool {
    // 0.0.0.0/8 — "this" network
    if octets[0] == 0 {
        return true;
    }
    // 10.0.0.0/8 — private
    if octets[0] == 10 {
        return true;
    }
    // 127.0.0.0/8 — loopback
    if octets[0] == 127 {
        return true;
    }
    // 169.254.0.0/16 — link-local
    if octets[0] == 169 && octets[1] == 254 {
        return true;
    }
    // 172.16.0.0/12 — private
    if octets[0] == 172 && (octets[1] & 0xF0) == 16 {
        return true;
    }
    // 192.168.0.0/16 — private
    if octets[0] == 192 && octets[1] == 168 {
        return true;
    }

    false
}

const fn is_cgnat_or_test_ipv4(octets: [u8; 4]) -> bool {
    // 100.64.0.0/10 — CGNAT (Carrier-Grade NAT)
    if octets[0] == 100 && (octets[1] & 0xC0) == 64 {
        return true;
    }
    // 192.0.2.0/24 — TEST-NET-1 (documentation)
    if octets[0] == 192 && octets[1] == 0 && octets[2] == 2 {
        return true;
    }
    // 198.18.0.0/15 — benchmark testing
    if octets[0] == 198 && (octets[1] & 0xFE) == 18 {
        return true;
    }
    // 198.51.100.0/24 — TEST-NET-2
    if octets[0] == 198 && octets[1] == 51 && octets[2] == 100 {
        return true;
    }
    // 203.0.113.0/24 — TEST-NET-3
    if octets[0] == 203 && octets[1] == 0 && octets[2] == 113 {
        return true;
    }

    false
}

const fn is_multicast_or_reserved_ipv4(octets: [u8; 4]) -> bool {
    // 224.0.0.0/4 — multicast
    if (octets[0] & 0xF0) == 224 {
        return true;
    }
    // 240.0.0.0/4 — reserved (includes 255.255.255.255 broadcast)
    if (octets[0] & 0xF0) == 240 {
        return true;
    }

    false
}

/// Extracts an embedded IPv4 address from an IPv6 address if one of the
/// known transition mechanisms is detected.
///
/// Handles: `::ffff:x.x.x.x` (mapped), `64:ff9b::x.x.x.x` (NAT64),
/// `2002:xxxx::`  (6to4), `2001:0000::` (Teredo with XOR decode),
/// `::x.x.x.x` (IPv4-compatible, deprecated).
fn extract_embedded_ipv4(ip: &Ipv6Addr) -> Option<Ipv4Addr> {
    let segments = ip.segments();
    let octets = ip.octets();

    // ::ffff:x.x.x.x — IPv4-mapped IPv6
    if let Some(mapped) = ip.to_ipv4_mapped() {
        return Some(mapped);
    }

    // 64:ff9b::x.x.x.x — NAT64 well-known prefix (RFC 6052 §2.1.1),
    // also covers the locale-dependent Network-Specific Prefix variants
    // (`64:ff9b:1::/48`, `2001:db8::/32` documentation, etc.) — RFC 6052
    // §2.2 says the prefix family is variable and the first 32 bits
    // (`64:ff9b:0000`) are the constant identifier. Match the upper 32
    // bits only; the lower 64 bits hold the embedded IPv4 per the spec.
    if segments[0] == 0x0064
        && segments[1] == 0xff9b
        && segments[2] == 0
        && segments[3] == 0
    {
        return Some(Ipv4Addr::new(
            octets[12], octets[13], octets[14], octets[15],
        ));
    }

    // 2002:xxxx:xxxx:: — 6to4 (RFC 3056), IPv4 in bits 16-47
    if segments[0] == 0x2002 {
        return Some(Ipv4Addr::new(octets[2], octets[3], octets[4], octets[5]));
    }

    // 2001:0000:: — Teredo (RFC 4380), IPv4 is XOR'd with 0xFFFFFFFF in last 32 bits
    if segments[0] == 0x2001 && segments[1] == 0x0000 {
        return Some(Ipv4Addr::new(
            octets[12] ^ 0xFF,
            octets[13] ^ 0xFF,
            octets[14] ^ 0xFF,
            octets[15] ^ 0xFF,
        ));
    }

    // ::x.x.x.x — IPv4-compatible (deprecated, RFC 4291 section 2.5.5.1)
    // First 96 bits (segments 0–5) are zero; last 32 bits (segments 6–7) hold the IPv4.
    // The trailing != 0 guard excludes :: and ::1, which are handled by
    // is_loopback / is_unspecified; checking both low segments ensures forms like
    // ::0.0.x.y (segments[6] == 0, segments[7] != 0) are still extracted.
    if segments[0..6] == [0, 0, 0, 0, 0, 0] && (segments[6] != 0 || segments[7] != 0) {
        return Some(Ipv4Addr::new(
            octets[12], octets[13], octets[14], octets[15],
        ));
    }

    None
}

/// Returns true if the IPv6 address falls in any blocked range.
///
/// Blocks: `::1` (loopback), :: (unspecified), `fe80::/10` (link-local),
/// `fc00::/7` (unique local), `ff00::/8` (multicast).
/// Also extracts and validates embedded IPv4 addresses from transition mechanisms.
pub(crate) fn is_blocked_ipv6(ip: Ipv6Addr) -> bool {
    // ::1 — loopback
    if ip.is_loopback() {
        return true;
    }
    // :: — unspecified
    if ip.is_unspecified() {
        return true;
    }

    let segments = ip.segments();

    // fe80::/10 — link-local
    if (segments[0] & 0xFFC0) == 0xFE80 {
        return true;
    }
    // fc00::/7 — unique local address
    if (segments[0] & 0xFE00) == 0xFC00 {
        return true;
    }
    // ff00::/8 — multicast
    if (segments[0] & 0xFF00) == 0xFF00 {
        return true;
    }

    // Check embedded IPv4 from transition mechanisms
    if let Some(embedded_v4) = extract_embedded_ipv4(&ip) {
        return is_blocked_ipv4(embedded_v4);
    }

    false
}

/// Returns true if the IP address should be blocked (delegates to v4/v6).
pub(crate) fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_blocked_ipv4(v4),
        IpAddr::V6(v6) => is_blocked_ipv6(v6),
    }
}

/// IPv4 cloud instance-metadata endpoint (AWS/Azure/GCP/OpenStack all share it).
const METADATA_IPV4: Ipv4Addr = Ipv4Addr::new(169, 254, 169, 254);

/// AWS IMDS over IPv6 (`fd00:ec2::254`, RFC 4193 unique-local). Reachable when
/// `allow_private_network` opens `fc00::/7`, so it needs an explicit floor.
const METADATA_IPV6: Ipv6Addr = Ipv6Addr::new(0xfd00, 0x0ec2, 0, 0, 0, 0, 0, 0x0254);

/// Returns true for cloud instance-metadata service addresses — the classic
/// SSRF pivot. These stay blocked even when private networks are permitted,
/// including when the metadata IPv4 is smuggled inside an IPv6 transition form.
pub(crate) fn is_cloud_metadata(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4 == METADATA_IPV4,
        IpAddr::V6(v6) => {
            if v6 == METADATA_IPV6 {
                return true;
            }
            if let Some(mapped) = v6.to_ipv4_mapped() {
                return mapped == METADATA_IPV4;
            }
            if let Some(embedded) = extract_embedded_ipv4(&v6) {
                return embedded == METADATA_IPV4;
            }
            false
        }
    }
}

/// Returns true if the IP resolves (directly or via an IPv6 transition form) to
/// the loopback range.
fn is_policy_loopback(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback(),
        IpAddr::V6(v6) => {
            if v6.is_loopback() {
                return true;
            }
            if let Some(mapped) = v6.to_ipv4_mapped() {
                return mapped.is_loopback();
            }
            if let Some(embedded) = extract_embedded_ipv4(&v6) {
                return embedded.is_loopback();
            }
            false
        }
    }
}

/// Returns true if the IP should be blocked under the given policy.
///
/// When `allow_private_network` is true, only loopback and cloud metadata
/// (169.254.169.254 / `fd00:ec2::254`) are blocked — the non-negotiable floor.
/// Otherwise the full blocklist applies. When the policy is disabled
/// (`enabled == false`), all IPs are allowed.
pub(crate) fn is_ip_blocked_by_policy(ip: IpAddr, policy: &SsrfPolicy) -> bool {
    if !policy.enabled {
        return false;
    }
    if policy.allow_private_network {
        // Floor that holds even for trusted internal hosts: never reach the
        // loopback interface or a cloud metadata endpoint.
        is_policy_loopback(ip) || is_cloud_metadata(ip)
    } else {
        is_blocked_ip(ip)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- IPv4 blocked ranges ---

    #[test]
    fn blocks_this_network() {
        assert!(is_blocked_ipv4(Ipv4Addr::new(0, 0, 0, 0)));
        assert!(is_blocked_ipv4(Ipv4Addr::new(0, 255, 255, 255)));
    }

    #[test]
    fn blocks_private_10() {
        assert!(is_blocked_ipv4(Ipv4Addr::new(10, 0, 0, 1)));
        assert!(is_blocked_ipv4(Ipv4Addr::new(10, 255, 255, 255)));
    }

    #[test]
    fn blocks_cgnat() {
        assert!(is_blocked_ipv4(Ipv4Addr::new(100, 64, 0, 1)));
        assert!(is_blocked_ipv4(Ipv4Addr::new(100, 127, 255, 255)));
        // Just outside CGNAT range
        assert!(!is_blocked_ipv4(Ipv4Addr::new(100, 128, 0, 0)));
    }

    #[test]
    fn blocks_loopback_v4() {
        assert!(is_blocked_ipv4(Ipv4Addr::new(127, 0, 0, 1)));
        assert!(is_blocked_ipv4(Ipv4Addr::new(127, 255, 255, 255)));
    }

    #[test]
    fn blocks_link_local_v4() {
        assert!(is_blocked_ipv4(Ipv4Addr::new(169, 254, 0, 1)));
        assert!(is_blocked_ipv4(Ipv4Addr::new(169, 254, 169, 254)));
    }

    #[test]
    fn blocks_private_172() {
        assert!(is_blocked_ipv4(Ipv4Addr::new(172, 16, 0, 1)));
        assert!(is_blocked_ipv4(Ipv4Addr::new(172, 31, 255, 255)));
        // Just outside
        assert!(!is_blocked_ipv4(Ipv4Addr::new(172, 32, 0, 1)));
    }

    #[test]
    fn blocks_test_net_1() {
        assert!(is_blocked_ipv4(Ipv4Addr::new(192, 0, 2, 1)));
        assert!(is_blocked_ipv4(Ipv4Addr::new(192, 0, 2, 255)));
    }

    #[test]
    fn blocks_private_192() {
        assert!(is_blocked_ipv4(Ipv4Addr::new(192, 168, 0, 1)));
        assert!(is_blocked_ipv4(Ipv4Addr::new(192, 168, 255, 255)));
    }

    #[test]
    fn blocks_benchmark() {
        assert!(is_blocked_ipv4(Ipv4Addr::new(198, 18, 0, 1)));
        assert!(is_blocked_ipv4(Ipv4Addr::new(198, 19, 255, 255)));
        // Just outside
        assert!(!is_blocked_ipv4(Ipv4Addr::new(198, 20, 0, 0)));
    }

    #[test]
    fn blocks_test_net_2() {
        assert!(is_blocked_ipv4(Ipv4Addr::new(198, 51, 100, 0)));
        assert!(is_blocked_ipv4(Ipv4Addr::new(198, 51, 100, 255)));
    }

    #[test]
    fn blocks_test_net_3() {
        assert!(is_blocked_ipv4(Ipv4Addr::new(203, 0, 113, 0)));
        assert!(is_blocked_ipv4(Ipv4Addr::new(203, 0, 113, 255)));
    }

    #[test]
    fn blocks_multicast_v4() {
        assert!(is_blocked_ipv4(Ipv4Addr::new(224, 0, 0, 1)));
        assert!(is_blocked_ipv4(Ipv4Addr::new(239, 255, 255, 255)));
    }

    #[test]
    fn blocks_reserved_v4() {
        assert!(is_blocked_ipv4(Ipv4Addr::new(240, 0, 0, 1)));
        assert!(is_blocked_ipv4(Ipv4Addr::new(255, 255, 255, 255)));
    }

    #[test]
    fn allows_public_ipv4() {
        assert!(!is_blocked_ipv4(Ipv4Addr::new(8, 8, 8, 8)));
        assert!(!is_blocked_ipv4(Ipv4Addr::new(1, 1, 1, 1)));
        assert!(!is_blocked_ipv4(Ipv4Addr::new(93, 184, 216, 34)));
    }

    // --- IPv6 blocked ranges ---

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
        assert!(is_blocked_ipv6("fe80::1".parse().unwrap()));
        assert!(is_blocked_ipv6("fe80::dead:beef".parse().unwrap()));
    }

    #[test]
    fn blocks_ipv6_unique_local() {
        assert!(is_blocked_ipv6("fc00::1".parse().unwrap()));
        assert!(is_blocked_ipv6("fd00::1".parse().unwrap()));
    }

    #[test]
    fn blocks_ipv6_multicast() {
        assert!(is_blocked_ipv6("ff02::1".parse().unwrap()));
        assert!(is_blocked_ipv6("ff05::1".parse().unwrap()));
    }

    // --- Embedded IPv4 in IPv6 ---

    #[test]
    fn blocks_ipv4_mapped_ipv6_loopback() {
        // ::ffff:127.0.0.1
        let addr: Ipv6Addr = "::ffff:127.0.0.1".parse().unwrap();
        assert!(is_blocked_ipv6(addr));
    }

    #[test]
    fn blocks_ipv4_mapped_ipv6_private() {
        let addr: Ipv6Addr = "::ffff:10.0.0.1".parse().unwrap();
        assert!(is_blocked_ipv6(addr));
    }

    #[test]
    fn allows_ipv4_mapped_ipv6_public() {
        let addr: Ipv6Addr = "::ffff:8.8.8.8".parse().unwrap();
        assert!(!is_blocked_ipv6(addr));
    }

    #[test]
    fn blocks_nat64_private() {
        // 64:ff9b::10.0.0.1
        let addr: Ipv6Addr = "64:ff9b::10.0.0.1".parse().unwrap();
        assert!(is_blocked_ipv6(addr));
    }

    #[test]
    fn blocks_6to4_private() {
        // 2002:0a00:0001:: embeds 10.0.0.1
        let addr: Ipv6Addr = "2002:0a00:0001::".parse().unwrap();
        assert!(is_blocked_ipv6(addr));
    }

    #[test]
    fn blocks_teredo_loopback() {
        // Teredo: 2001:0000:xxxx:xxxx:xxxx:xxxx:YYYY:ZZZZ
        // IPv4 = YYYY ^ FFFF : ZZZZ ^ FFFF
        // For 127.0.0.1: XOR with FFFF gives 0x80ff:fffe
        let addr: Ipv6Addr = "2001:0000:0000:0000:0000:0000:80ff:fffe".parse().unwrap();
        assert!(is_blocked_ipv6(addr));
    }

    #[test]
    fn allows_public_ipv6() {
        // Google public DNS IPv6
        let addr: Ipv6Addr = "2001:4860:4860::8888".parse().unwrap();
        assert!(!is_blocked_ipv6(addr));
    }

    #[test]
    fn blocks_ipv4_compatible_loopback() {
        let addr: Ipv6Addr = "::127.0.0.1".parse().unwrap();
        assert!(is_blocked_ipv6(addr));
    }

    #[test]
    fn blocks_ipv4_compatible_private() {
        let addr: Ipv6Addr = "::10.0.0.1".parse().unwrap();
        assert!(is_blocked_ipv6(addr));
    }

    #[test]
    fn allows_ipv4_compatible_public() {
        let addr: Ipv6Addr = "::8.8.8.8".parse().unwrap();
        assert!(!is_blocked_ipv6(addr));
    }

    // --- Policy-aware checks ---

    #[test]
    fn policy_default_blocks_private() {
        let policy = SsrfPolicy::default();
        assert!(is_ip_blocked_by_policy(
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)),
            &policy
        ));
    }

    #[test]
    fn policy_allow_private_permits_private_ip() {
        let policy = SsrfPolicy {
            allow_private_network: true,
            ..Default::default()
        };
        assert!(!is_ip_blocked_by_policy(
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)),
            &policy
        ));
    }

    #[test]
    fn policy_allow_private_still_blocks_loopback() {
        let policy = SsrfPolicy {
            allow_private_network: true,
            ..Default::default()
        };
        assert!(is_ip_blocked_by_policy(
            IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
            &policy
        ));
    }

    #[test]
    fn policy_allow_private_still_blocks_cloud_metadata() {
        let policy = SsrfPolicy {
            allow_private_network: true,
            ..Default::default()
        };
        assert!(is_ip_blocked_by_policy(
            IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254)),
            &policy
        ));
    }

    #[test]
    fn policy_allow_private_still_blocks_ipv6_metadata() {
        // fc00::/7 (unique-local) is permitted under allow_private_network, but
        // the AWS IMDS IPv6 endpoint inside it must stay blocked.
        let policy = SsrfPolicy {
            allow_private_network: true,
            ..Default::default()
        };
        assert!(is_ip_blocked_by_policy(
            IpAddr::V6(Ipv6Addr::new(0xfd00, 0x0ec2, 0, 0, 0, 0, 0, 0x0254)),
            &policy
        ));
        // A sibling ULA address (not metadata) remains reachable.
        assert!(!is_ip_blocked_by_policy(
            IpAddr::V6(Ipv6Addr::new(0xfd00, 0x1234, 0, 0, 0, 0, 0, 0x0001)),
            &policy
        ));
    }

    #[test]
    fn policy_allow_private_blocks_metadata_via_ipv4_mapped() {
        // 169.254.169.254 smuggled as an IPv4-mapped IPv6 address.
        let policy = SsrfPolicy {
            allow_private_network: true,
            ..Default::default()
        };
        let mapped = Ipv4Addr::new(169, 254, 169, 254).to_ipv6_mapped();
        assert!(is_ip_blocked_by_policy(IpAddr::V6(mapped), &policy));
    }
}
