//! Native TLS termination for the gateway.
//!
//! Two ways in, one knob for the user (`[gateway.tls] mode = "auto"`):
//!
//! - **BYO cert** — both `cert_path` and `key_path` set ⇒ load them (a real
//!   CA-signed cert, e.g. from your reverse proxy or ACME client). Setting only
//!   one is a hard error, never a silent downgrade.
//! - **Self-signed autogen** — otherwise ⇒ generate a self-signed cert once and
//!   cache it under `~/.aleph/gateway/tls/`. Framed for local/LAN use: browsers
//!   show a one-time "proceed" warning; native clients pin the SHA-256
//!   fingerprint (logged at startup).
//!
//! We deliberately do **not** ship ACME/Let's Encrypt (see openclaw/hermes: public
//! TLS is the reverse proxy's job — [`crate::gateway::trusted_proxy`]). The crypto
//! provider is `ring` (Cargo.toml pins `tls-rustls-no-provider` + a self-installed
//! ring provider) so the heavy `aws-lc-rs` native build stays out of the tree.

use anyhow::{bail, Context};
use axum_server::tls_rustls::RustlsConfig;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::net::{Ipv4Addr, Ipv6Addr};
use std::path::{Path, PathBuf};
use std::sync::Once;

/// Self-signed certificate validity, in days from generation. Kept under
/// Apple's 398-day cap so Safari / macOS (Aleph's primary platform) accept the
/// cert instead of rejecting it outright, as they do for rcgen's default
/// 1975→4096 range.
const SELF_SIGNED_VALIDITY_DAYS: i64 = 397;

/// TLS mode for the gateway listener.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TlsMode {
    /// Plaintext HTTP/WS. Default — zero behavior change on upgrade.
    #[default]
    Off,
    /// Serve HTTPS/WSS: BYO cert if `cert_path`+`key_path` are set, else a
    /// cached self-signed cert.
    Auto,
}

/// `[gateway.tls]` configuration. Absent table ⇒ [`TlsMode::Off`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TlsConfig {
    /// `"off"` (default) or `"auto"`.
    pub mode: TlsMode,
    /// PEM certificate (chain) path for a bring-your-own cert. Requires
    /// `key_path`; setting only one is a hard error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cert_path: Option<PathBuf>,
    /// PEM private-key path for a bring-your-own cert. Requires `cert_path`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_path: Option<PathBuf>,
}

impl TlsConfig {
    /// Whether the gateway should serve TLS.
    #[must_use]
    pub const fn is_enabled(&self) -> bool {
        matches!(self.mode, TlsMode::Auto)
    }
}

/// Loaded TLS material handed to the server: the rustls config for the listener
/// plus the leaf certificate's SHA-256 fingerprint (logged for pinning).
pub struct GatewayTls {
    /// rustls config for `axum_server::bind_rustls`.
    pub rustls_config: RustlsConfig,
    /// Lowercase-hex SHA-256 of the DER leaf certificate, or `"unknown"` when a
    /// BYO PEM could not be parsed (non-fatal — CA certs are trusted by chain,
    /// not by pin).
    pub fingerprint_sha256: String,
}

/// Install the process-wide `ring` rustls crypto provider exactly once.
///
/// `RustlsConfig::from_pem` (built with `tls-rustls-no-provider`) needs a
/// process default provider. Installing is idempotent; a second attempt fails
/// harmlessly if one is already installed.
fn ensure_ring_provider() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// Load or generate the gateway TLS material.
///
/// `cache_dir` is where the self-signed pair is cached (created if missing,
/// key written `0600` on Unix). The self-signed SANs are the fixed loopback set
/// — browsers proceed past the self-signed warning regardless of SAN, and
/// native clients pin the fingerprint (which ignores SANs), so the cert never
/// needs regenerating when the bind host changes.
pub async fn load_or_generate(config: &TlsConfig, cache_dir: &Path) -> anyhow::Result<GatewayTls> {
    ensure_ring_provider();

    let (cert_pem, key_pem) = match (&config.cert_path, &config.key_path) {
        (Some(cert), Some(key)) => {
            let cert_pem = tokio::fs::read(cert)
                .await
                .with_context(|| format!("reading TLS cert {}", cert.display()))?;
            let key_pem = tokio::fs::read(key)
                .await
                .with_context(|| format!("reading TLS key {}", key.display()))?;
            (cert_pem, key_pem)
        }
        (None, None) => load_or_generate_self_signed(cache_dir).await?,
        _ => bail!(
            "[gateway.tls] cert_path and key_path must both be set for a bring-your-own cert \
             (found only one); leave both unset for a self-signed cert"
        ),
    };

    let fingerprint_sha256 = fingerprint_from_pem(&cert_pem);
    let rustls_config = RustlsConfig::from_pem(cert_pem, key_pem)
        .await
        .context("building rustls config from PEM (cert/key mismatch or bad PEM?)")?;

    Ok(GatewayTls {
        rustls_config,
        fingerprint_sha256,
    })
}

/// Load a cached self-signed pair, or generate + cache one. A cache that fails
/// to parse (torn write, corruption) self-heals by regenerating.
async fn load_or_generate_self_signed(cache_dir: &Path) -> anyhow::Result<(Vec<u8>, Vec<u8>)> {
    let cert_path = cache_dir.join("cert.pem");
    let key_path = cache_dir.join("key.pem");

    if let (Ok(cert), Ok(key)) = (
        tokio::fs::read(&cert_path).await,
        tokio::fs::read(&key_path).await,
    ) {
        if pem_has_tag(&cert, "CERTIFICATE") && pem_has_tag(&key, "PRIVATE KEY") {
            // Re-assert owner-only perms on the key in case an older build (or a
            // manual copy) left it world-readable.
            reassert_private(&key_path).await;
            return Ok((cert, key));
        }
        tracing::warn!(
            dir = %cache_dir.display(),
            "cached self-signed TLS material did not parse; regenerating"
        );
    }

    let (cert_pem, key_pem) = generate_self_signed()?;

    tokio::fs::create_dir_all(cache_dir)
        .await
        .with_context(|| format!("creating TLS cache dir {}", cache_dir.display()))?;
    write_private(&cert_path, &cert_pem).await?;
    write_private(&key_path, &key_pem).await?;

    Ok((cert_pem, key_pem))
}

/// Generate a fresh self-signed cert + key (PEM), valid for the fixed loopback
/// SAN set with an Apple-compatible validity window.
fn generate_self_signed() -> anyhow::Result<(Vec<u8>, Vec<u8>)> {
    use time::{Duration, OffsetDateTime};

    let sans = vec![
        "localhost".to_string(),
        Ipv4Addr::LOCALHOST.to_string(),
        Ipv6Addr::LOCALHOST.to_string(),
    ];
    let mut params =
        rcgen::CertificateParams::new(sans).context("building self-signed cert params")?;
    let now = OffsetDateTime::now_utc();
    params.not_before = now - Duration::days(1);
    params.not_after = now + Duration::days(SELF_SIGNED_VALIDITY_DAYS);

    let key = rcgen::KeyPair::generate().context("generating TLS key pair")?;
    let cert = params
        .self_signed(&key)
        .context("self-signing gateway certificate")?;

    Ok((cert.pem().into_bytes(), key.serialize_pem().into_bytes()))
}

/// Write `data` to `path`, creating it owner-only (`0600`) on Unix *before* any
/// bytes land (no world-readable window).
async fn write_private(path: &Path, data: &[u8]) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use tokio::io::AsyncWriteExt;
        let mut f = tokio::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .await
            .with_context(|| format!("creating {}", path.display()))?;
        f.write_all(data)
            .await
            .with_context(|| format!("writing {}", path.display()))?;
        f.flush().await.ok();
    }
    #[cfg(not(unix))]
    {
        tokio::fs::write(path, data)
            .await
            .with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(())
}

/// Best-effort re-assert of owner-only perms on a cached key (Unix only).
async fn reassert_private(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).await;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

/// Whether `pem_bytes` contains at least one block with the given tag.
fn pem_has_tag(pem_bytes: &[u8], tag: &str) -> bool {
    pem::parse_many(pem_bytes)
        .map(|blocks| blocks.iter().any(|b| b.tag() == tag))
        .unwrap_or(false)
}

/// SHA-256 (lowercase hex) of the first `CERTIFICATE` block's DER in `cert_pem`.
/// Returns `"unknown"` when no certificate block parses.
fn fingerprint_from_pem(cert_pem: &[u8]) -> String {
    let Ok(blocks) = pem::parse_many(cert_pem) else {
        return "unknown".to_string();
    };
    match blocks.into_iter().find(|b| b.tag() == "CERTIFICATE") {
        Some(block) => hex::encode(Sha256::digest(block.contents())),
        None => "unknown".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_off_by_default_and_not_enabled() {
        let c = TlsConfig::default();
        assert_eq!(c.mode, TlsMode::Off);
        assert!(!c.is_enabled());
    }

    #[test]
    fn mode_auto_is_enabled() {
        let c = TlsConfig {
            mode: TlsMode::Auto,
            ..Default::default()
        };
        assert!(c.is_enabled());
    }

    #[test]
    fn generated_cert_validity_is_apple_compatible() {
        use time::OffsetDateTime;
        let (cert_pem, _key) = generate_self_signed().unwrap();
        let block = pem::parse_many(&cert_pem)
            .unwrap()
            .into_iter()
            .find(|b| b.tag() == "CERTIFICATE")
            .unwrap();
        let (_, cert) = x509_parser::parse_x509_certificate(block.contents()).unwrap();
        let not_before = cert.validity().not_before.timestamp();
        let not_after = cert.validity().not_after.timestamp();
        let span_days = (not_after - not_before) / 86_400;
        assert!(
            span_days <= 398,
            "self-signed validity {span_days}d exceeds Apple's 398-day cap"
        );
        // Valid right now.
        let now = OffsetDateTime::now_utc().unix_timestamp();
        assert!(not_before <= now && now <= not_after);
    }

    #[test]
    fn fingerprint_of_generated_cert_is_stable_hex() {
        let (cert_pem, _key) = generate_self_signed().unwrap();
        let fp = fingerprint_from_pem(&cert_pem);
        assert_ne!(fp, "unknown");
        assert_eq!(fp.len(), 64, "SHA-256 hex is 64 chars");
        assert!(fp.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(fp, fingerprint_from_pem(&cert_pem));
    }

    #[test]
    fn fingerprint_of_non_cert_pem_is_unknown() {
        assert_eq!(fingerprint_from_pem(b"not a pem"), "unknown");
    }

    #[tokio::test]
    async fn self_signed_generate_then_reuse_cache() {
        let dir = std::env::temp_dir().join(format!("aleph-tls-test-{}", std::process::id()));
        let _ = tokio::fs::remove_dir_all(&dir).await;

        let (cert1, key1) = load_or_generate_self_signed(&dir).await.unwrap();
        assert!(!cert1.is_empty() && !key1.is_empty());
        // Second call reuses the cache byte-for-byte.
        let (cert2, key2) = load_or_generate_self_signed(&dir).await.unwrap();
        assert_eq!(cert1, cert2);
        assert_eq!(key1, key2);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let meta = tokio::fs::metadata(dir.join("key.pem")).await.unwrap();
            assert_eq!(meta.permissions().mode() & 0o777, 0o600);
        }
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn corrupt_cache_self_heals() {
        let dir = std::env::temp_dir().join(format!("aleph-tls-corrupt-{}", std::process::id()));
        let _ = tokio::fs::remove_dir_all(&dir).await;
        tokio::fs::create_dir_all(&dir).await.unwrap();
        tokio::fs::write(dir.join("cert.pem"), b"garbage")
            .await
            .unwrap();
        tokio::fs::write(dir.join("key.pem"), b"garbage")
            .await
            .unwrap();

        // Regenerates rather than returning the corrupt bytes.
        let (cert, key) = load_or_generate_self_signed(&dir).await.unwrap();
        assert!(pem_has_tag(&cert, "CERTIFICATE"));
        assert!(pem_has_tag(&key, "PRIVATE KEY"));
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn byo_half_config_is_hard_error() {
        let cfg = TlsConfig {
            mode: TlsMode::Auto,
            cert_path: Some(PathBuf::from("/nonexistent/cert.pem")),
            key_path: None,
        };
        let dir = std::env::temp_dir();
        let res = load_or_generate(&cfg, &dir).await;
        assert!(res.is_err());
        assert!(res.err().unwrap().to_string().contains("both be set"));
    }
}
