//! Shared local-server startup I/O, with no runtime or process at import.
//! HTTP readiness is not binary identity or permission to stop a service.
#[cfg(feature = "gateway-admission")]
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::net::TcpStream;

const DAEMON_HOST: &str = "127.0.0.1";
const DAEMON_PORT: u16 = 18790;
/// How long to wait for the daemon to report ready before giving up.
const READY_TIMEOUT: Duration = Duration::from_secs(40);
/// Interval between `/ready` polls.
const POLL_INTERVAL: Duration = Duration::from_millis(400);
/// Upper bound on a single localhost HTTP probe (connect + write + read).
/// A process that accepts the connection but never replies cannot hang it.
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
/// Winsock can take just over two seconds to report a refused loopback
/// connection. Allow that explicit refusal without widening the HTTP probe.
const CONNECT_TIMEOUT: Duration = if cfg!(windows) {
    Duration::from_secs(4)
} else {
    PROBE_TIMEOUT
};

#[cfg(feature = "gateway-admission")]
mod admission;

/// Admit an existing listener or start the sibling binary on a refused port.
/// Keep the child handle even if the subsequent readiness check fails.
#[cfg(feature = "gateway-admission")]
pub async fn ensure_compatible_ready(owned: &mut Option<BundledServer>) -> Result<(), String> {
    admission::ensure(DAEMON_PORT, async || {
        if let Some(previous) = owned {
            match previous.0.try_wait() {
                Ok(Some(_)) => {}
                Ok(None) => {
                    return Err("bundled Aleph is still running without a ready listener".into())
                }
                Err(error) => return Err(format!("could not check bundled Aleph: {error}")),
            }
        }
        let bin = bundled_daemon_binary()
            .ok_or_else(|| "could not locate the bundled aleph-server binary".to_string())?;
        *owned = Some(BundledServer::spawn(&bin)?);
        Ok(())
    })
    .await
}

// Some(false) proves refusal; None is unknown and must not grant a launch.
fn classify_connect(result: Result<(), std::io::ErrorKind>) -> Option<bool> {
    match result {
        Ok(()) => Some(true),
        Err(std::io::ErrorKind::ConnectionRefused) => Some(false),
        Err(_) => None,
    }
}

async fn port_open(port: u16) -> Option<bool> {
    let result = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect((DAEMON_HOST, port)))
        .await
        .ok()?;
    classify_connect(result.map(|_| ()).map_err(|error| error.kind()))
}

/// Platform-specific daemon binary file name.
#[cfg(feature = "gateway-admission")]
const fn daemon_bin_name() -> &'static str {
    if cfg!(windows) {
        "aleph-server.exe"
    } else {
        "aleph-server"
    }
}

#[cfg(feature = "gateway-admission")]
fn bundled_daemon_binary() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let candidate = exe.parent()?.join(daemon_bin_name());
    candidate.is_file().then_some(candidate)
}

/// The child and the write end of its private stop pipe belong to the host
/// that spawned it. Ordinary Quit closes the pipe without requesting stop.
#[cfg(feature = "gateway-admission")]
pub struct BundledServer(std::process::Child);

#[cfg(feature = "gateway-admission")]
impl BundledServer {
    fn spawn(bin: &Path) -> Result<Self, String> {
        use std::process::{Command, Stdio};

        let mut command = Command::new(bin);
        command
            .args([
                "--bundled-content-only",
                "--desktop-shutdown-stdin",
                "start",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const DETACHED_PROCESS: u32 = 0x0000_0008;
            const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
            command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
        }
        command.spawn().map(Self).map_err(|error| {
            format!(
                "could not start bundled Aleph at {}: {error}",
                bin.display()
            )
        })
    }

    /// Requests orderly shutdown and waits for the actual child to exit.
    pub fn stop(&mut self) -> Result<(), String> {
        use std::io::Write;
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        let mut pipe_error = None;
        if let Some(mut stdin) = self.0.stdin.take() {
            let request = [aleph_protocol::panel_shell::DESKTOP_SHUTDOWN_BYTE];
            if let Err(error) = stdin.write_all(&request) {
                pipe_error = Some(error);
                self.0.stdin = Some(stdin);
            }
        }
        loop {
            match self.0.try_wait() {
                Ok(Some(_)) => return Ok(()),
                Ok(None) => {}
                Err(error) => return Err(format!("could not wait for bundled Aleph: {error}")),
            }
            if std::time::Instant::now() >= deadline {
                return Err(match pipe_error {
                    Some(error) => format!("could not request bundled Aleph shutdown: {error}"),
                    None => "bundled Aleph did not exit after the shutdown request".to_owned(),
                });
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connect_errors_are_not_free_port_evidence() {
        use std::io::ErrorKind;

        for error in [
            ErrorKind::TimedOut,
            ErrorKind::PermissionDenied,
            ErrorKind::ConnectionReset,
            ErrorKind::Interrupted,
            ErrorKind::Other,
        ] {
            assert_eq!(classify_connect(Err(error)), None);
        }
        assert_eq!(classify_connect(Ok(())), Some(true));
        assert_eq!(
            classify_connect(Err(ErrorKind::ConnectionRefused)),
            Some(false)
        );
    }

    #[tokio::test]
    async fn port_presence_observes_owned_listener_and_non_listener() {
        let listener = tokio::net::TcpListener::bind((DAEMON_HOST, 0))
            .await
            .unwrap();
        assert_eq!(
            port_open(listener.local_addr().unwrap().port()).await,
            Some(true)
        );

        // Keep the endpoint bound but not listening, so no other fixture can
        // acquire it. The OS may refuse or time out; derive that observation
        // independently instead of assuming drop means immediate refusal.
        let closed = tokio::net::TcpSocket::new_v4().unwrap();
        closed
            .bind((std::net::Ipv4Addr::LOCALHOST, 0).into())
            .unwrap();
        let address = closed.local_addr().unwrap();
        let observed = std::net::TcpStream::connect_timeout(&address, Duration::from_secs(6));
        assert!(
            observed.is_err(),
            "a non-listening fixture accepted a connection"
        );
        assert_eq!(
            port_open(address.port()).await,
            match observed.unwrap_err().kind() {
                std::io::ErrorKind::ConnectionRefused => Some(false),
                _ => None,
            }
        );
    }

    #[cfg(all(unix, feature = "gateway-admission"))]
    #[test]
    fn stop_waits_for_the_child_receiving_the_private_request() {
        use std::process::{Command, Stdio};

        let child = Command::new("/bin/sh")
            .args([
                "-c",
                r#"IFS= read -r request || exit 7; test -z "$request""#,
            ])
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        let mut server = BundledServer(child);
        server.stop().unwrap();
        assert!(server.0.try_wait().unwrap().unwrap().success());
    }
}
