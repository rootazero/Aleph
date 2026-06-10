//! `.ipc-endpoint.json` write + read helpers.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::warn;

const ENDPOINT_FILENAME: &str = ".ipc-endpoint.json";

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
            version: 1,
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
    Ok(Some(ep))
}

pub fn remove_endpoint(data_dir: &Path) {
    let path = endpoint_path(data_dir);
    if let Err(e) = std::fs::remove_file(&path) {
        if e.kind() != std::io::ErrorKind::NotFound {
            warn!(path = %path.display(), error = %e, "failed to remove endpoint file");
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
        remove_endpoint(dir.path());
        assert!(read_endpoint(dir.path()).unwrap().is_none());
    }
}
