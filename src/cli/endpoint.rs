//! `.ipc-endpoint.json` write + read helpers.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::warn;

const ENDPOINT_FILENAME: &str = ".ipc-endpoint.json";
const CURRENT_ENDPOINT_VERSION: u32 = 1;

/// Build the IPC endpoint URL for a server bound on `addr`.
///
/// Rules:
/// - `tls = true` → `https://`, else `http://`. The CLI's HTTP client
///   (`ipc_client::build_client`) accepts self-signed certs **only** when
///   the URL host is loopback; a non-loopback `https://` URL is refused
///   outright. If you need a non-loopback admin channel, terminate TLS
///   at a CA-signed proxy in front of the server instead.
/// - `0.0.0.0` is a wildcard for IPv4 — map to `127.0.0.1` so a client
///   that is also on the box can reach the loopback listener.
/// - `::` is the IPv6 wildcard — map to `::1`.
/// - Concrete addresses are preserved verbatim.
pub fn build_endpoint_url(addr: SocketAddr, tls: bool) -> String {
    let scheme = if tls { "https" } else { "http" };
    let host = match addr.ip() {
        IpAddr::V4(Ipv4Addr::UNSPECIFIED) => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(Ipv6Addr::UNSPECIFIED) => IpAddr::V6(Ipv6Addr::LOCALHOST),
        other => other,
    };
    let host_str = match host {
        IpAddr::V4(v4) => v4.to_string(),
        IpAddr::V6(_) => format!("[{host}]"),
    };
    format!("{scheme}://{host_str}:{}", addr.port())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IpcEndpoint {
    pub version: u32,
    pub url: String,
    pub pid: u32,
    pub started_at: String,
}

impl IpcEndpoint {
    pub fn current(url: impl Into<String>) -> Self {
        Self {
            version: CURRENT_ENDPOINT_VERSION,
            url: url.into(),
            pid: std::process::id(),
            started_at: chrono::Utc::now().to_rfc3339(),
        }
    }
}

#[must_use]
pub(crate) fn endpoint_path(data_dir: &Path) -> PathBuf {
    data_dir.join(ENDPOINT_FILENAME)
}

pub fn write_endpoint(data_dir: &Path, endpoint: &IpcEndpoint) -> std::io::Result<()> {
    let path = endpoint_path(data_dir);
    let bytes = serde_json::to_vec_pretty(endpoint)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    crate::utils::atomic_io::write_atomic(&path, &bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perm = std::fs::Permissions::from_mode(0o600);
        if let Err(e) = std::fs::set_permissions(&path, perm) {
            // Don't leave a possibly world-readable file on disk if the
            // chmod step fails — the endpoint URL and PID are sensitive and
            // a half-secured file is worse than a missing one.
            let _ = std::fs::remove_file(&path);
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!("failed to restrict endpoint file permissions: {e}"),
            ));
        }
    }
    Ok(())
}

const MAX_ENDPOINT_FILE_SIZE: u64 = 1_048_576;

pub fn read_endpoint(data_dir: &Path) -> std::io::Result<Option<IpcEndpoint>> {
    use std::io::Read;

    let path = endpoint_path(data_dir);
    let file = match std::fs::File::open(&path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };

    // Bound the read to MAX+1 bytes so a corrupt or oversized file cannot
    // force an unbounded allocation BEFORE the size check fires. (The
    // previous `std::fs::read` loaded the whole file into memory first,
    // making the limit decorative.) Reading one byte past the limit lets us
    // detect "too big" without trusting metadata, which is racy against a
    // concurrent writer.
    let mut bytes = Vec::new();
    file.take(MAX_ENDPOINT_FILE_SIZE + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_ENDPOINT_FILE_SIZE {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(".ipc-endpoint.json exceeds size limit (> {MAX_ENDPOINT_FILE_SIZE} bytes)"),
        ));
    }
    // An empty file is the result of either a crashed half-write (which
    // `write_atomic` is supposed to make impossible — readers always see a
    // complete file or no file — but legacy installs may have inherited one
    // from a pre-atomic version, or an operator may have truncated the
    // file). Both cases should behave like "no endpoint" rather than a
    // JSON parse error surfacing to the caller: the IPC channel has no
    // useful information either way, and the next `start` overwrites the
    // file. Matches the version-mismatch branch below on purpose.
    if bytes.is_empty() {
        tracing::debug!(".ipc-endpoint.json is empty; treating as missing");
        return Ok(None);
    }
    let ep: IpcEndpoint = serde_json::from_slice(&bytes)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    if ep.version != CURRENT_ENDPOINT_VERSION {
        // A version bump makes an old endpoint file unreadable, not invalid:
        // treat it as "no endpoint" so the next `start` recreates it cleanly,
        // instead of hard-erroring every status/IPC call during an upgrade.
        tracing::debug!(
            found = ep.version,
            expected = CURRENT_ENDPOINT_VERSION,
            ".ipc-endpoint.json has unsupported version; treating as missing"
        );
        return Ok(None);
    }
    Ok(Some(ep))
}

pub fn remove_endpoint(data_dir: &Path) -> std::io::Result<()> {
    let path = endpoint_path(data_dir);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => {
            warn!(path = %path.display(), error = %e, "failed to remove endpoint file");
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_write_read() {
        let dir = tempfile::tempdir().unwrap();
        let ep = IpcEndpoint::current("http://127.0.0.1:9000");
        write_endpoint(dir.path(), &ep).unwrap();
        let read = read_endpoint(dir.path()).unwrap().unwrap();
        assert_eq!(read.url, "http://127.0.0.1:9000");
        assert_eq!(read.pid, std::process::id());
    }

    #[test]
    fn read_returns_none_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_endpoint(dir.path()).unwrap().is_none());
    }

    /// Regression: a 0-byte endpoint file is the signature of either a
    /// pre-atomic half-write legacy install or an operator-truncated file.
    /// Both should surface as "no endpoint" to keep `forward_to_server`'s
    /// friendly "server is initializing or crashed" path, not a confusing
    /// JSON parse error.
    #[test]
    fn read_returns_none_when_empty() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(endpoint_path(dir.path()), b"").unwrap();
        assert!(
            read_endpoint(dir.path()).unwrap().is_none(),
            "empty .ipc-endpoint.json must read as Ok(None)"
        );
    }

    /// Regression: a version-mismatched endpoint file is the result of an
    /// in-flight upgrade. `read_endpoint` returns Ok(None) so the next
    /// `start` overwrites the file cleanly rather than every status/IPC
    /// call hard-erroring during the transition.
    #[test]
    fn read_returns_none_for_unsupported_version() {
        let dir = tempfile::tempdir().unwrap();
        // Schema-valid JSON for IpcEndpoint but version != CURRENT.
        std::fs::write(
            endpoint_path(dir.path()),
            br#"{"version":999,"url":"http://127.0.0.1:9000","pid":1,"started_at":"2026-01-01T00:00:00Z"}"#,
        )
        .unwrap();
        assert!(
            read_endpoint(dir.path()).unwrap().is_none(),
            "unsupported endpoint version must read as Ok(None)"
        );
    }

    #[test]
    fn remove_cleans_file() {
        let dir = tempfile::tempdir().unwrap();
        let ep = IpcEndpoint::current("http://x");
        write_endpoint(dir.path(), &ep).unwrap();
        remove_endpoint(dir.path()).unwrap();
        assert!(read_endpoint(dir.path()).unwrap().is_none());
    }

    fn sa_v4(host: &str, port: u16) -> SocketAddr {
        SocketAddr::new(IpAddr::V4(host.parse().unwrap()), port)
    }

    fn sa_v6(host: &str, port: u16) -> SocketAddr {
        SocketAddr::new(IpAddr::V6(host.parse().unwrap()), port)
    }

    #[test]
    fn url_uses_https_when_tls_enabled() {
        let url = build_endpoint_url(sa_v4("127.0.0.1", 9000), true);
        assert_eq!(url, "https://127.0.0.1:9000");
    }

    #[test]
    fn url_uses_http_when_tls_disabled() {
        let url = build_endpoint_url(sa_v4("127.0.0.1", 9000), false);
        assert_eq!(url, "http://127.0.0.1:9000");
    }

    #[test]
    fn url_maps_ipv4_unspecified_to_loopback() {
        let url = build_endpoint_url(sa_v4("0.0.0.0", 18790), false);
        assert_eq!(url, "http://127.0.0.1:18790");
    }

    #[test]
    fn url_maps_ipv6_unspecified_to_ipv6_loopback() {
        let url = build_endpoint_url(sa_v6("::", 18790), false);
        assert_eq!(url, "http://[::1]:18790");
    }

    #[test]
    fn url_preserves_concrete_ipv4() {
        let url = build_endpoint_url(sa_v4("192.168.1.5", 9000), false);
        assert_eq!(url, "http://192.168.1.5:9000");
    }

    #[test]
    fn url_preserves_concrete_ipv6_global() {
        let url = build_endpoint_url(sa_v6("2001:db8::1", 9000), true);
        assert_eq!(url, "https://[2001:db8::1]:9000");
    }

    #[test]
    fn url_treats_explicit_loopback_as_concrete_ipv6() {
        let url = build_endpoint_url(sa_v6("::1", 9000), false);
        assert_eq!(url, "http://[::1]:9000");
    }

    #[cfg(unix)]
    #[test]
    fn write_endpoint_sets_owner_only_permissions() {
        // The success-path contract: the file is created with mode 0o600
        // on Unix. The chmod-failure path is reviewed by code inspection
        // (we cannot reliably simulate a chmod failure on the
        // filesystems that unit tests run on) — the relevant guarantee
        // here is that the *successful* path tightens perms.
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let ep = IpcEndpoint::current("http://127.0.0.1:9000");
        write_endpoint(dir.path(), &ep).unwrap();
        let mode = std::fs::metadata(endpoint_path(dir.path()))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "endpoint file must be owner-only readable");
    }
}
