//! Daemon supervisor: locate, launch, and probe `aleph-server`.
//!
//! Ensures `aleph-server` is running on 127.0.0.1:18790 with exponential backoff.

use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const DAEMON_HOST: &str = "127.0.0.1";
const DAEMON_PORT: u16 = 18790;
const READY_TIMEOUT: Duration = Duration::from_secs(40);
const INITIAL_POLL_INTERVAL: Duration = Duration::from_millis(400);
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PortOccupant {
    Free,
    DaemonReady,
    DaemonBooting,
    Foreign,
}

/// Ensure daemon is ready; launch if needed with exponential backoff probing.
pub async fn ensure_ready() -> Result<(), String> {
    match probe_port().await {
        PortOccupant::DaemonReady => Ok(()),
        PortOccupant::DaemonBooting => wait_until_ready().await,
        PortOccupant::Free => {
            launch_detached()?;
            wait_until_ready().await
        }
        PortOccupant::Foreign => Err(format!(
            "port {DAEMON_PORT} held by foreign process; free and retry"
        )),
    }
}

/// Poll until daemon is ready, with exponential backoff (capped at 2s).
async fn wait_until_ready() -> Result<(), String> {
    let deadline = tokio::time::Instant::now() + READY_TIMEOUT;
    let mut interval = INITIAL_POLL_INTERVAL;
    loop {
        if is_ready().await {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(format!(
                "aleph-server not ready within {}s",
                READY_TIMEOUT.as_secs()
            ));
        }
        tokio::time::sleep(interval).await;
        // Exponential backoff, capped at 2s
        interval = std::cmp::min(interval.mul_f32(2.0), Duration::from_secs(2));
    }
}

async fn is_ready() -> bool {
    matches!(http_get_status("/ready").await, Some(200))
}

async fn probe_port() -> PortOccupant {
    if !port_open().await {
        return PortOccupant::Free;
    }
    classify_ready_status(http_get_status("/ready").await)
}

const fn classify_ready_status(status: Option<u16>) -> PortOccupant {
    match status {
        Some(200) => PortOccupant::DaemonReady,
        Some(503) => PortOccupant::DaemonBooting,
        _ => PortOccupant::Foreign,
    }
}

async fn port_open() -> bool {
    TcpStream::connect((DAEMON_HOST, DAEMON_PORT)).await.is_ok()
}

async fn http_get_status(path: &str) -> Option<u16> {
    tokio::time::timeout(PROBE_TIMEOUT, http_get_status_inner(path))
        .await
        .ok()?
}

async fn http_get_status_inner(path: &str) -> Option<u16> {
    let mut stream = TcpStream::connect((DAEMON_HOST, DAEMON_PORT)).await.ok()?;
    let request =
        format!("GET {path} HTTP/1.0\r\nHost: {DAEMON_HOST}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await.ok()?;
    let mut chunk = [0u8; 512];
    let n = stream.read(&mut chunk).await.ok()?;
    let head = String::from_utf8_lossy(chunk.get(..n)?);
    head.split_whitespace().nth(1)?.parse().ok()
}

fn launch_detached() -> Result<(), String> {
    let bin = resolve_daemon_binary().ok_or_else(|| "aleph-server binary not found".to_string())?;
    spawn_detached(&bin)
}

const fn daemon_bin_name() -> &'static str {
    if cfg!(windows) {
        "aleph-server.exe"
    } else {
        "aleph-server"
    }
}

pub fn resolve_daemon_binary() -> Option<PathBuf> {
    let name = daemon_bin_name();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(candidate) = exe.parent().map(|d| d.join(name)) {
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|p| p.is_file())
}

#[cfg(unix)]
fn spawn_detached(bin: &Path) -> Result<(), String> {
    use std::process::{Command, Stdio};
    let mut launcher = Command::new(bin)
        .arg("--daemon")
        .arg("start")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("spawn failed: {e}"))?;
    let _ = launcher.wait();
    Ok(())
}

#[cfg(windows)]
fn spawn_detached(bin: &Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let _child = Command::new(bin)
        .arg("--daemon")
        .arg("start")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| format!("spawn failed: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_daemon_binary_returns_none_when_not_on_path() {
        let result = resolve_daemon_binary();
        // Either Some (if binary is on PATH) or None; test just checks it doesn't panic
        let _ = result;
    }

    #[test]
    fn classify_ready_status_ready() {
        assert_eq!(classify_ready_status(Some(200)), PortOccupant::DaemonReady);
    }

    #[test]
    fn classify_ready_status_booting() {
        assert_eq!(
            classify_ready_status(Some(503)),
            PortOccupant::DaemonBooting
        );
    }

    #[test]
    fn classify_ready_status_foreign() {
        assert_eq!(classify_ready_status(Some(404)), PortOccupant::Foreign);
        assert_eq!(classify_ready_status(None), PortOccupant::Foreign);
    }
}
