//! Native in-process TLS material for the gateway listener.
//!
//! Three modes off `[gateway.tls]`: disabled (plaintext), operator-provided
//! cert/key files, or auto self-signed (generated once via `rcgen`, persisted
//! to `~/.aleph/data/tls/`, fingerprint printed for client pinning). No ACME
//! here — auto-issuance is Caddy's / certbot's job (R3).

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
}
