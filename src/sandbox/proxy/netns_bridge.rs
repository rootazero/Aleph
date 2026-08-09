//! Phase B — netns → UDS → loopback bridge for Linux `NetworkPolicy::AllowHosts`.
//!
//! # Why
//!
//! The managed proxy ([`super::lifecycle`]) listens on the **host's**
//! `127.0.0.1:N` and enforces the hostname allowlist. On macOS Seatbelt the
//! sandboxed process shares that loopback, so injecting `HTTP_PROXY` is
//! enough. On Linux, `bwrap --unshare-net` gives the sandbox its **own**
//! empty network namespace whose loopback is a different `127.0.0.1` — the
//! host proxy is unreachable from inside.
//!
//! A Unix-domain socket is a *filesystem* object, not a network resource, so
//! it crosses the netns boundary when bind-mounted into the sandbox. Phase B
//! chains two tiny byte-shovels around that UDS:
//!
//! ```text
//! target (in netns)  --HTTP_PROXY=127.0.0.1:LOCAL-->  local bridge (in netns)
//!                                                          |  connect(UDS)
//!                                  [UDS bind-mounted; crosses the netns edge]
//!                                                          v
//!   managed proxy (host) <--connect(127.0.0.1:PROXY)--  host bridge (host)
//! ```
//!
//! This module is the **host half** (the part running inside `aleph-server`).
//! Unlike codex's `proxy_routing.rs`, which `fork()`s a host bridge from a
//! single-threaded helper, `aleph-server` is a multi-threaded Tokio process
//! where `fork()` is unsafe — so the host bridge is an async task using
//! [`tokio::io::copy_bidirectional`]. The **netns half** (the local bridge
//! forked inside the single-threaded `sandbox-init` helper) lives in
//! [`crate::sandbox::sandbox_init`].
//!
//! The route spec ([`ProxyRouteSpec`]) is the contract between the host and
//! the netns: it travels as a JSON env var ([`PROXY_ROUTE_SPEC_ENV`]) that the
//! bwrap driver reads to bind-mount the UDS directory and `sandbox-init` reads
//! to spawn the local bridge and rewrite the proxy env vars.

use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Env var carrying the serialized [`ProxyRouteSpec`] from the host into the
/// sandbox. Read (and then stripped) by `sandbox-init` before the untrusted
/// target execs, so the target never sees the routing internals.
pub const PROXY_ROUTE_SPEC_ENV: &str = "ALEPH_SANDBOX_PROXY_ROUTE_SPEC";

/// Per-run private directory prefix under the socket parent dir. Includes the
/// owning PID so stale dirs from crashed runs can be reaped.
const PROXY_SOCKET_DIR_PREFIX: &str = "aleph-sandbox-proxy-";

/// Contract between the host (`aleph-server`) and the netns (`sandbox-init`).
///
/// - `uds_path`: the Unix-domain socket the host bridge listens on; the bwrap
///   driver bind-mounts its parent directory into the sandbox at the same
///   path, and the netns-side local bridge connects to it.
/// - `env_keys`: the proxy env vars (`HTTP_PROXY`, …) whose loopback **port**
///   `sandbox-init` rewrites to the local bridge's port (scheme + structure
///   preserved). Their pre-rewrite value points at the host proxy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxyRouteSpec {
    pub uds_path: PathBuf,
    pub env_keys: Vec<String>,
}

impl ProxyRouteSpec {
    /// Serialize for transport via [`PROXY_ROUTE_SPEC_ENV`].
    pub fn to_env_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// Parse from the [`PROXY_ROUTE_SPEC_ENV`] value.
    pub fn from_env_json(value: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(value)
    }
}

/// Parent directory for per-run socket dirs. Prefers `~/.aleph/tmp` (a
/// private, predictable location whose 0700 perms we can guarantee); falls
/// back to the system temp dir if home is undiscoverable. Mirrors codex's
/// `proxy_socket_parent_dir` intent (private parent, short paths under
/// `sun_path`'s 108-byte limit).
pub fn proxy_socket_parent_dir() -> PathBuf {
    if let Some(home) = dirs::home_dir() {
        let candidate = home.join(".aleph").join("tmp");
        if ensure_private_dir(&candidate).is_ok() {
            return candidate;
        }
    }
    std::env::temp_dir()
}

fn ensure_private_dir(path: &Path) -> io::Result<()> {
    use std::fs::DirBuilder;
    #[cfg(unix)]
    use std::os::unix::fs::DirBuilderExt;

    let mut builder = DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    builder.mode(0o700);
    match builder.create(path) {
        Ok(()) => {}
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {}
        Err(err) => return Err(err),
    }
    #[cfg(unix)]
    {
        use std::fs::Permissions;
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Create a fresh per-run private (0700) directory to hold the bridge UDS.
/// The caller owns cleanup (the returned [`HostBridgeHandle`] removes it on
/// drop). Before allocating, reaps stale dirs whose owner PID is gone.
pub fn create_proxy_socket_dir() -> io::Result<PathBuf> {
    use std::fs::DirBuilder;
    #[cfg(unix)]
    use std::os::unix::fs::DirBuilderExt;

    let parent = proxy_socket_parent_dir();
    let _ = reap_stale_socket_dirs(&parent);

    let pid = std::process::id();
    for attempt in 0..128 {
        let candidate = parent.join(format!("{PROXY_SOCKET_DIR_PREFIX}{pid}-{attempt}"));
        // `mut` is only needed where the Unix `mode()` call below mutates it.
        #[cfg_attr(not(unix), allow(unused_mut))]
        let mut builder = DirBuilder::new();
        #[cfg(unix)]
        builder.mode(0o700);
        match builder.create(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!(
            "failed to allocate proxy socket dir under {}",
            parent.display()
        ),
    ))
}

/// Best-effort removal of per-run dirs whose owner PID is no longer alive.
/// Never fails the run — bridge setup proceeds even if cleanup hiccups.
fn reap_stale_socket_dirs(parent: &Path) -> io::Result<()> {
    for entry in std::fs::read_dir(parent)? {
        let Ok(entry) = entry else { continue };
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some(owner_pid) = parse_socket_dir_owner_pid(name.as_ref()) else {
            continue;
        };
        if is_pid_alive(owner_pid) {
            continue;
        }
        let _ = std::fs::remove_dir_all(entry.path());
    }
    Ok(())
}

fn parse_socket_dir_owner_pid(name: &str) -> Option<u32> {
    let suffix = name.strip_prefix(PROXY_SOCKET_DIR_PREFIX)?;
    let (pid_raw, _) = suffix.split_once('-')?;
    pid_raw.parse::<u32>().ok().filter(|pid| *pid != 0)
}

#[cfg(unix)]
fn is_pid_alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    // SAFETY: `kill(pid, 0)` probes process existence without signal delivery.
    // rust-doctor-disable-next-line unsafe-block-audit
    let rc = unsafe { libc::kill(pid, 0) };
    if rc == 0 {
        return true;
    }
    // ESRCH = no such process → dead. EPERM = alive but not ours → alive.
    !matches!(io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH))
}

#[cfg(not(unix))]
fn is_pid_alive(_pid: u32) -> bool {
    true
}

/// Handle to a running host bridge. Dropping aborts the accept loop and
/// removes the per-run socket directory (and the UDS within it).
pub struct HostBridgeHandle {
    socket_dir: PathBuf,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for HostBridgeHandle {
    fn drop(&mut self) {
        self.task.abort();
        let _ = std::fs::remove_dir_all(&self.socket_dir);
    }
}

impl std::fmt::Debug for HostBridgeHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostBridgeHandle")
            .field("socket_dir", &self.socket_dir)
            .finish()
    }
}

/// Spawn the host bridge: bind a Unix listener at `uds_path` and forward every
/// accepted connection to `upstream` (the host managed proxy) via
/// [`tokio::io::copy_bidirectional`]. Returns once the listener is bound, so a
/// caller that races to bind-mount the UDS into the sandbox is safe.
///
/// `socket_dir` is the per-run private directory containing `uds_path`; the
/// returned handle owns its cleanup.
///
/// Unix-domain sockets are a Unix-only `tokio` feature. The host bridge only
/// matters for the Linux `bwrap` netns routing path (the `workspace.rs` caller
/// invokes it solely when the platform is `linux/bwrap`), so on non-Unix
/// targets this is a never-reached stub that keeps the public API present.
#[cfg(unix)]
pub async fn spawn_host_bridge(
    uds_path: &Path,
    socket_dir: PathBuf,
    upstream: SocketAddr,
) -> io::Result<HostBridgeHandle> {
    use tokio::net::UnixListener;

    // A leftover socket inode would make bind() fail with EADDRINUSE; the
    // per-run dir is fresh, but be defensive against a reused path.
    if uds_path.exists() {
        let _ = tokio::fs::remove_file(uds_path).await;
    }
    let listener = UnixListener::bind(uds_path)?;

    let task = tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((uds_stream, _)) => {
                    tokio::spawn(async move {
                        if let Err(e) = bridge_connection(uds_stream, upstream).await {
                            tracing::debug!(
                                target = "sandbox.proxy",
                                error = %e,
                                "host bridge connection ended with error"
                            );
                        }
                    });
                }
                Err(e) => {
                    tracing::debug!(
                        target = "sandbox.proxy",
                        error = %e,
                        "host bridge accept failed; backing off"
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
            }
        }
    });

    Ok(HostBridgeHandle { socket_dir, task })
}

/// Non-Unix stub. `tokio`'s `UnixListener`/`UnixStream` do not exist on
/// Windows, and the host bridge is only ever spawned on the Linux `bwrap`
/// path, so this never runs at runtime — it exists only so `alephcore`
/// compiles for Windows (and other non-Unix) targets.
#[cfg(not(unix))]
pub async fn spawn_host_bridge(
    _uds_path: &Path,
    _socket_dir: PathBuf,
    _upstream: SocketAddr,
) -> io::Result<HostBridgeHandle> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "netns host bridge is only available on Unix (Linux bwrap netns routing)",
    ))
}

#[cfg(unix)]
async fn bridge_connection(
    mut uds_stream: tokio::net::UnixStream,
    upstream: SocketAddr,
) -> io::Result<()> {
    let mut tcp_stream = tokio::net::TcpStream::connect(upstream).await?;
    tokio::io::copy_bidirectional(&mut uds_stream, &mut tcp_stream).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn route_spec_round_trips_through_json() {
        let spec = ProxyRouteSpec {
            uds_path: PathBuf::from("/run/aleph/proxy-0.sock"),
            env_keys: vec!["HTTP_PROXY".into(), "HTTPS_PROXY".into()],
        };
        let json = spec.to_env_json().unwrap();
        let parsed = ProxyRouteSpec::from_env_json(&json).unwrap();
        assert_eq!(spec, parsed);
    }

    #[test]
    fn parse_socket_dir_owner_pid_reads_pid() {
        assert_eq!(
            parse_socket_dir_owner_pid("aleph-sandbox-proxy-4321-0"),
            Some(4321)
        );
        assert_eq!(parse_socket_dir_owner_pid("aleph-sandbox-proxy-x-0"), None);
        assert_eq!(parse_socket_dir_owner_pid("unrelated-dir"), None);
    }

    #[test]
    fn create_socket_dir_is_private_and_reused_pid_isolated() {
        let dir = create_proxy_socket_dir().expect("socket dir should create");
        assert!(dir.is_dir());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&dir).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700, "socket dir must be 0700");
        }
        // Two allocations never collide.
        let dir2 = create_proxy_socket_dir().expect("second socket dir");
        assert_ne!(dir, dir2);
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dir2);
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn host_bridge_forwards_bytes_uds_to_tcp() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        // `create_proxy_socket_dir()` derives the UDS path from $HOME (via
        // `dirs::home_dir()`). Other tests mutate HOME to a long temp path;
        // serialize against them so we bind under the real (short) HOME and
        // never overflow the platform `SUN_LEN` limit.
        let _home_guard = crate::runtimes::post_install::HomeEnvGuard::acquire();

        // Upstream: a loopback TCP echo-once server standing in for the
        // managed proxy.
        let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_addr = upstream.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut s, _)) = upstream.accept().await {
                let mut buf = [0u8; 4];
                let _ = s.read_exact(&mut buf).await;
                let _ = s.write_all(&buf).await;
                let _ = s.shutdown().await;
            }
        });

        let socket_dir = create_proxy_socket_dir().unwrap();
        let uds_path = socket_dir.join("proxy-0.sock");
        let handle = spawn_host_bridge(&uds_path, socket_dir.clone(), upstream_addr)
            .await
            .unwrap();

        let mut client = tokio::net::UnixStream::connect(&uds_path).await.unwrap();
        client.write_all(b"ping").await.unwrap();
        let mut got = [0u8; 4];
        client.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"ping", "bytes must round-trip UDS→TCP→UDS");

        // Drop cleans up the socket dir.
        drop(handle);
        // Give the abort + rmdir a beat.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(!socket_dir.exists(), "drop must remove the socket dir");
    }
}
