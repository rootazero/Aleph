//! Native in-process TLS material for the gateway listener.
//!
//! Three modes off `[gateway.tls]`: disabled (plaintext), operator-provided
//! cert/key files, or auto self-signed (generated once via `rcgen`, persisted
//! to `~/.aleph/data/tls/`, fingerprint printed for client pinning). No ACME
//! here — auto-issuance is Caddy's / certbot's job (R3).

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
            if cert_file.exists() && key_file.exists() {
                let cert = tokio::fs::read(&cert_file).await?;
                let key = tokio::fs::read(&key_file).await?;
                let fp = fingerprint(&cert);
                return Ok((cert, key, fp));
            }
            let (cert, key, fp) = generate_self_signed()?;
            tokio::fs::create_dir_all(dir).await?;
            tokio::fs::write(&cert_file, &cert).await?;
            tokio::fs::write(&key_file, &key).await?;
            Ok((cert, key, fp))
        }
    }
}

/// rcgen 0.13 self-signed for localhost + loopback. Returns PEM cert, PEM key,
/// and the SHA-256 fingerprint hex of the PEM cert bytes.
fn generate_self_signed() -> anyhow::Result<(Vec<u8>, Vec<u8>, String)> {
    let rcgen::CertifiedKey { cert, key_pair } = rcgen::generate_simple_self_signed(vec![
        "localhost".to_string(),
        "127.0.0.1".to_string(),
    ])?;
    let cert_pem = cert.pem().into_bytes();
    let key_pem = key_pair.serialize_pem().into_bytes();
    let fp = fingerprint(&cert_pem);
    Ok((cert_pem, key_pem, fp))
}

/// Base SANs every self-signed cert always carries.
const BASE_SANS: [&str; 3] = ["localhost", "127.0.0.1", "::1"];

/// True if `s` is a plausible DNS name. rcgen rejects garbage and would fail
/// cert generation, which must never brick startup, so we pre-filter.
fn is_plausible_dns_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 253
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
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
}
