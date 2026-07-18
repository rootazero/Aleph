//! Native in-process TLS material for the gateway listener.
//!
//! Three modes off `[gateway.tls]`: disabled (plaintext), operator-provided
//! cert/key files, or auto self-signed. The self-signed cert's SAN covers
//! loopback, every non-loopback interface IP (auto-discovered), and any
//! `[gateway.tls] san` extras; it is persisted to `~/.aleph/data/tls/` with a
//! `sans.txt` sidecar, and regenerated when a newly-desired SAN is not yet
//! covered. No ACME here — auto-issuance is Caddy's / certbot's job (R3).

use std::collections::HashSet;
use std::net::IpAddr;
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::gateway::config::GatewayTlsConfig;

/// Which TLS material the listener should use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TlsMode {
    Disabled,
    Provided { cert_path: String, key_path: String },
    SelfSigned,
}

/// Resolve the mode from config (pure).
#[must_use]
pub fn resolve_mode(cfg: &GatewayTlsConfig) -> TlsMode {
    if !cfg.enabled {
        return TlsMode::Disabled;
    }
    if !cfg.cert_path.is_empty() && !cfg.key_path.is_empty() {
        return TlsMode::Provided {
            cert_path: cfg.cert_path.clone(),
            key_path: cfg.key_path.clone(),
        };
    }
    TlsMode::SelfSigned
}

/// Return `(cert_pem, key_pem, sha256_fingerprint_hex)` for the resolved mode.
/// For `SelfSigned`, generate-and-persist under `dir`, reusing an existing pair.
pub async fn load_or_generate(
    cfg: &GatewayTlsConfig,
    dir: &Path,
) -> anyhow::Result<(Vec<u8>, Vec<u8>, String)> {
    match resolve_mode(cfg) {
        TlsMode::Disabled => anyhow::bail!("TLS disabled"),
        TlsMode::Provided { cert_path, key_path } => {
            let cert = tokio::fs::read(&cert_path).await?;
            let key = tokio::fs::read(&key_path).await?;
            let fp = fingerprint(&cert);
            Ok((cert, key, fp))
        }
        TlsMode::SelfSigned => {
            let cert_file = dir.join("cert.pem");
            let key_file = dir.join("key.pem");
            let san_file = dir.join("sans.txt");

            let discovered = discover_interface_ips();
            let desired = self_signed_sans(&cfg.san, &discovered);

            if cert_file.exists() && key_file.exists() && san_file.exists() {
                // Best-effort: a corrupt / unreadable sidecar yields an empty set,
                // which fails the subset check and falls through to regeneration
                // rather than bricking TLS startup.
                let recorded = tokio::fs::read_to_string(&san_file)
                    .await
                    .map(|s| parse_recorded_sans(&s))
                    .unwrap_or_default();
                if desired_covered(&recorded, &desired) {
                    let cert = tokio::fs::read(&cert_file).await?;
                    let key = tokio::fs::read(&key_file).await?;
                    let fp = fingerprint(&cert);
                    return Ok((cert, key, fp));
                }
            }

            let (cert, key, fp) = generate_self_signed(&desired)?;
            tokio::fs::create_dir_all(dir).await?;
            // `sans.txt` is the reuse commit marker: remove it BEFORE overwriting
            // cert/key so a crash mid-rewrite (cert written, key not yet) leaves no
            // marker — the next boot regenerates a fresh matching pair instead of
            // reusing a torn cert/key that would fail `from_pem`. Written last. (P7)
            let _ = tokio::fs::remove_file(&san_file).await;
            tokio::fs::write(&cert_file, &cert).await?;
            tokio::fs::write(&key_file, &key).await?;
            tokio::fs::write(&san_file, desired.join("\n")).await?;
            Ok((cert, key, fp))
        }
    }
}

/// rcgen 0.13 self-signed for the given SANs (each string classified as IP or
/// DNS by rcgen). Returns PEM cert, PEM key, and the SHA-256 fingerprint hex.
fn generate_self_signed(sans: &[String]) -> anyhow::Result<(Vec<u8>, Vec<u8>, String)> {
    let rcgen::CertifiedKey { cert, key_pair } =
        rcgen::generate_simple_self_signed(sans.to_vec())?;
    let cert_pem = cert.pem().into_bytes();
    let key_pem = key_pair.serialize_pem().into_bytes();
    let fp = fingerprint(&cert_pem);
    Ok((cert_pem, key_pem, fp))
}

/// Base SANs every self-signed cert always carries.
const BASE_SANS: [&str; 3] = ["localhost", "127.0.0.1", "::1"];

/// True if `s` is a plausible DNS name. rcgen accepts arbitrary ASCII as a
/// `DnsName`, so this only trims obvious junk (empty / non-DNS chars) to avoid
/// shipping meaningless SANs; it never bricks startup.
fn is_plausible_dns_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 253
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
}

/// Parse the newline-delimited SAN sidecar into a set (blank lines ignored).
fn parse_recorded_sans(content: &str) -> HashSet<String> {
    content.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect()
}

/// Reuse the persisted cert iff every desired SAN is already recorded (subset,
/// not equality — removing an interface IP must not thrash the cert).
fn desired_covered(recorded: &HashSet<String>, desired: &[String]) -> bool {
    desired.iter().all(|s| recorded.contains(s))
}

/// True if `ip` is a usable SAN target (not loopback, not link-local).
fn is_usable_san_ip(ip: &IpAddr) -> bool {
    if ip.is_loopback() {
        return false;
    }
    match ip {
        IpAddr::V4(v4) => !v4.is_link_local(),
        // fe80::/10 link-local (is_unicast_link_local is unstable on MSRV 1.95).
        IpAddr::V6(v6) => (v6.segments()[0] & 0xffc0) != 0xfe80,
    }
}

/// Enumerate this host's usable non-loopback interface IPs. Best-effort: any
/// failure yields an empty vec (the cert still gets loopback + configured SANs).
pub(crate) fn discover_interface_ips() -> Vec<IpAddr> {
    match if_addrs::get_if_addrs() {
        Ok(ifaces) => ifaces.into_iter().map(|i| i.ip()).filter(is_usable_san_ip).collect(),
        Err(e) => {
            tracing::warn!(error = %e, "gateway.tls: interface discovery failed; SAN limited to loopback + config");
            Vec::new()
        }
    }
}

/// Assemble the SAN list for a self-signed cert (pure): base loopback set +
/// discovered interface IPs + validated operator extras, order-stable deduped.
pub(crate) fn self_signed_sans(configured: &[String], discovered: &[IpAddr]) -> Vec<String> {
    let mut sans: Vec<String> = BASE_SANS.iter().map(|s| (*s).to_string()).collect();
    for ip in discovered {
        sans.push(ip.to_string());
    }
    for raw in configured {
        let s = raw.trim();
        if s.parse::<IpAddr>().is_ok() || is_plausible_dns_name(s) {
            sans.push(s.to_string());
        } else if !s.is_empty() {
            tracing::warn!(san = %s, "gateway.tls.san: dropping malformed SAN entry");
        }
    }
    let mut seen = HashSet::new();
    sans.retain(|s| seen.insert(s.clone()));
    sans
}

fn fingerprint(cert_pem: &[u8]) -> String {
    let digest = Sha256::digest(cert_pem);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::config::GatewayTlsConfig;

    #[test]
    fn mode_resolution() {
        assert!(matches!(resolve_mode(&GatewayTlsConfig::default()), TlsMode::Disabled));

        let mut c = GatewayTlsConfig { enabled: true, ..Default::default() };
        assert!(matches!(resolve_mode(&c), TlsMode::SelfSigned));

        c.cert_path = "/a".into();
        c.key_path = "/b".into();
        assert!(matches!(resolve_mode(&c), TlsMode::Provided { .. }));
    }

    #[tokio::test]
    async fn self_signed_generates_persists_and_reuses() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = GatewayTlsConfig { enabled: true, ..Default::default() };

        let (cert1, key1, fp1) = load_or_generate(&cfg, dir.path()).await.unwrap();
        assert!(cert1.starts_with(b"-----BEGIN CERTIFICATE-----"));
        assert!(!key1.is_empty());
        assert_eq!(fp1.len(), 64); // hex SHA-256

        // Second call reuses the persisted cert → identical fingerprint.
        let (_c2, _k2, fp2) = load_or_generate(&cfg, dir.path()).await.unwrap();
        assert_eq!(fp1, fp2);
    }

    #[tokio::test]
    async fn shrinking_desired_set_reuses_without_regen() {
        let dir = tempfile::tempdir().unwrap();
        // Wide config records an extra SAN in sans.txt.
        let cfg_wide = GatewayTlsConfig {
            enabled: true,
            san: vec!["203.0.113.88".to_string()],
            ..Default::default()
        };
        let (_c0, _k0, fp0) = load_or_generate(&cfg_wide, dir.path()).await.unwrap();
        // Narrow config drops that SAN → desired shrinks but stays ⊆ recorded →
        // reuse, no regen, fingerprint unchanged (no thrash).
        let cfg_narrow = GatewayTlsConfig { enabled: true, ..Default::default() };
        let (_c1, _k1, fp1) = load_or_generate(&cfg_narrow, dir.path()).await.unwrap();
        assert_eq!(fp0, fp1, "a shrunk desired set must reuse the cert, not regenerate");
    }

    #[test]
    fn sans_include_base_discovered_and_config() {
        use std::net::{IpAddr, Ipv4Addr};
        let discovered = vec![IpAddr::V4(Ipv4Addr::new(172, 245, 43, 211))];
        let configured = vec!["vps.example.com".to_string(), "10.0.0.5".to_string()];
        let sans = self_signed_sans(&configured, &discovered);
        for expect in ["localhost", "127.0.0.1", "::1", "172.245.43.211", "vps.example.com", "10.0.0.5"] {
            assert!(sans.contains(&expect.to_string()), "missing {expect}");
        }
    }

    #[test]
    fn sans_dedup_and_drop_malformed() {
        use std::net::{IpAddr, Ipv4Addr};
        let discovered = vec![
            IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7)),
            IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7)),
        ];
        let configured = vec!["127.0.0.1".to_string(), "bad name!".to_string(), "   ".to_string()];
        let sans = self_signed_sans(&configured, &discovered);
        assert_eq!(sans.iter().filter(|s| *s == "203.0.113.7").count(), 1);
        assert_eq!(sans.iter().filter(|s| *s == "127.0.0.1").count(), 1);
        assert!(!sans.iter().any(|s| s.contains('!')));
    }

    #[test]
    fn usable_san_ip_filters_loopback_and_link_local() {
        use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
        assert!(!is_usable_san_ip(&IpAddr::V4(Ipv4Addr::LOCALHOST)));
        assert!(!is_usable_san_ip(&IpAddr::V4(Ipv4Addr::new(169, 254, 1, 1))));
        assert!(!is_usable_san_ip(&IpAddr::V6(Ipv6Addr::LOCALHOST)));
        assert!(!is_usable_san_ip(&IpAddr::V6(Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1))));
        assert!(is_usable_san_ip(&IpAddr::V4(Ipv4Addr::new(172, 245, 43, 211))));
        assert!(is_usable_san_ip(&IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1))));
    }

    #[test]
    fn discover_interface_ips_has_no_loopback_and_no_panic() {
        for ip in discover_interface_ips() {
            assert!(!ip.is_loopback());
        }
    }

    #[test]
    fn parse_recorded_sans_ignores_blanks() {
        let set = parse_recorded_sans("localhost\n127.0.0.1\n\n  \n203.0.113.7\n");
        assert_eq!(set.len(), 3);
        assert!(set.contains("203.0.113.7"));
    }

    #[test]
    fn desired_covered_subset_logic() {
        let recorded: HashSet<String> =
            ["localhost", "127.0.0.1", "::1", "203.0.113.7"].iter().map(|s| s.to_string()).collect();
        assert!(desired_covered(&recorded, &["127.0.0.1".to_string(), "203.0.113.7".to_string()]));
        assert!(!desired_covered(&recorded, &["203.0.113.7".to_string(), "198.51.100.9".to_string()]));
    }

    #[tokio::test]
    async fn regenerates_when_desired_not_covered() {
        let dir = tempfile::tempdir().unwrap();
        let cfg0 = GatewayTlsConfig { enabled: true, ..Default::default() };
        let (_c0, _k0, fp0) = load_or_generate(&cfg0, dir.path()).await.unwrap();
        assert!(dir.path().join("sans.txt").exists());

        let cfg1 = GatewayTlsConfig {
            enabled: true,
            san: vec!["203.0.113.77".to_string()],
            ..Default::default()
        };
        let (_c1, _k1, fp1) = load_or_generate(&cfg1, dir.path()).await.unwrap();
        assert_ne!(fp0, fp1, "adding an uncovered SAN must regenerate the cert");

        let recorded =
            parse_recorded_sans(&tokio::fs::read_to_string(dir.path().join("sans.txt")).await.unwrap());
        assert!(recorded.contains("203.0.113.77"));
    }
}
