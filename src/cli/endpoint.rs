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
/// - `tls = true` → `https://`, else `http://`. (Self-signed cert trust is
///   the caller's problem; this only formats the URL.)
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
pub fn endpoint_path(data_dir: &Path) -> PathBuf {
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
        std::fs::set_permissions(&path, perm).map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!("failed to restrict endpoint file permissions: {e}"),
            )
        })?;
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
}
