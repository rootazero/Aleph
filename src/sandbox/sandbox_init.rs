//! SP-2 init binary logic — applies landlock + seccomp inside the bwrap
//! mount namespace, then `execvp()`s the target program.
//!
//! Invoked as `aleph-server sandbox-init --policy <json> -- <target>
//! <target-args...>` by `BubblewrapDriver::run`. Lives in a hidden CLI
//! subcommand on the existing aleph-server binary (no separate helper
//! artifact — R3 core minimalism).
//!
//! The init prelude runs inside bwrap's namespace, *after* bwrap finishes
//! its mount/PID/user-ns setup and *before* the untrusted target gets to
//! execute even one instruction. That's the correct LSM hook point: the
//! filesystem is already constrained by mounts, but the kernel attack
//! surface and intra-mount FS ACL are not yet locked down.
//!
//! Cross-platform parts (policy struct, JSON shape, denylist constant,
//! capability→path translation) are not gated so they compile and unit-
//! test on macOS / Windows dev boxes. The actual `apply_*` and `run_init`
//! entry point are `#[cfg(target_os = "linux")]`-gated because they call
//! Linux-only kernel APIs.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::sandbox::capabilities::SandboxCapabilities;

/// Policy passed from `BubblewrapDriver::run` to `sandbox-init` via JSON
/// on argv. Bounded by capability count (typical < 4 KiB serialized;
/// well under `MAX_ARG_STRLEN`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LinuxInitPolicy {
    /// Paths to grant `READ_FILE | READ_DIR | EXECUTE` on (landlock).
    /// Always includes the system minimum (`/usr`, `/lib`, `/lib64`,
    /// `/bin`, `/sbin`, `/etc`) plus every `SandboxCapabilities.fs_read`
    /// entry.
    pub read_paths: Vec<PathBuf>,
    /// Paths to grant `READ_FILE | READ_DIR | WRITE_FILE | REMOVE_FILE |
    /// REMOVE_DIR | MAKE_REG | MAKE_DIR | MAKE_SYM | EXECUTE` on
    /// (landlock). One per `SandboxCapabilities.fs_write` entry; the
    /// workspace cwd is included automatically by the caller.
    pub write_paths: Vec<PathBuf>,
    /// When `true`, `apply_landlock` returns an error if the kernel
    /// does not expose landlock ABI ≥ 1. Default `false` → soft
    /// degrade with a warning.
    #[serde(default)]
    pub require_landlock: bool,

    /// Socket-family restriction applied by the seccomp filter. See
    /// [`SeccompNetworkMode`]. Defaults to [`SeccompNetworkMode::Unrestricted`]
    /// for forward-compatible deserialization of policies that predate the
    /// field (old wire shape used a `deny_network_sockets: bool`; an absent
    /// value maps to the old `false` = no socket filtering).
    #[serde(default)]
    pub seccomp_net: SeccompNetworkMode,
}

/// Type-safe model of the seccomp socket-family gate, mapping codex's
/// `NetworkSeccompMode` (Restricted / `ProxyRouted`) plus a third "no
/// filtering" state into a single enum so illegal combinations are
/// unrepresentable. The driver picks the variant from
/// [`SandboxCapabilities::network`]; `apply_seccomp` lowers it to concrete
/// `socket(2)` / `socketpair(2)` / `connect(2)` argument rules.
///
/// The bubblewrap `--unshare-net` netns already strips interfaces, so this
/// is defense-in-depth: it turns runtime socket failures into early `EPERM`
/// with a clear audit trail and, in `ProxyRouted` mode, narrows the socket
/// surface to exactly what the loopback proxy bridge needs.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SeccompNetworkMode {
    /// No socket-family filtering. The kernel namespace and any
    /// driver-level rules carry network policy. Used for `AllowAll` and
    /// raw (non-loopback) `AllowHosts`, where seccomp cannot filter by IP.
    #[default]
    Unrestricted,
    /// Network disabled (`NetworkPolicy::None`): allow only `AF_UNIX`
    /// sockets (needed for local IPC) and deny `connect(2)` outright.
    /// Mirrors codex `NetworkSeccompMode::Restricted`, minus the
    /// individual `accept`/`bind`/`sendto`/… denials that bwrap's empty
    /// netns already makes unreachable.
    UnixOnly,
    /// Proxied egress (loopback `AllowHosts` behind the netns→UDS→loopback
    /// bridge): allow only `AF_INET` / `AF_INET6` sockets (to reach the
    /// local TCP bridge on `lo`) and deny `AF_UNIX` socketpairs so the
    /// target cannot open a fresh Unix socket to sidestep the routed
    /// bridge. Mirrors codex `NetworkSeccompMode::ProxyRouted`. `connect`
    /// stays allowed — the target must reach `127.0.0.1:<bridge-port>`.
    ProxyRouted,
}

/// System-minimum read paths granted unconditionally. Without these,
/// dynamic linker / libc / shells in `/usr/bin` cannot load.
pub const SYSTEM_READ_PATHS: &[&str] = &["/usr", "/lib", "/lib64", "/bin", "/sbin", "/etc"];

/// Pseudo-device files granted read+write unconditionally. `apply_landlock`
/// handles `AccessFs::from_all`, which makes the ruleset default-deny for
/// every path not covered by an explicit rule — including the `/dev`
/// devtmpfs that bwrap mounts via `--dev /dev`. Without these grants, shell
/// redirection to `/dev/null`, `/dev/tty` writes, and similar pseudo-device
/// I/O fail with `EACCES`. bwrap exposes only a minimal devtmpfs (no block
/// devices), so granting the standard writable pseudo-devices is safe.
/// Mirrors codex's explicit `/dev/null` RW grant, widened to the customary
/// set. Absent entries are skipped (`PathFd::new` failure is non-fatal).
pub const SYSTEM_DEVICE_RW_PATHS: &[&str] = &["/dev/null", "/dev/zero", "/dev/full", "/dev/tty"];

/// Pseudo-device files granted read-only unconditionally. `/dev/urandom`
/// in particular is read by libc, language runtimes (Python `os.urandom`,
/// OpenSSL seeding) and most crypto — denying it breaks a large class of
/// otherwise-benign tools. Read-only because nothing legitimate writes the
/// kernel RNG from inside the sandbox.
pub const SYSTEM_DEVICE_RO_PATHS: &[&str] = &["/dev/random", "/dev/urandom"];

/// Frozen seccomp denylist. Each name corresponds to a syscall that gets
/// `SECCOMP_RET_ERRNO(EPERM)`; everything else falls through to allow.
/// `EPERM` (vs `SIGKILL`) keeps the program survivable so it can log /
/// report errors. Argument-aware filters for `clone` / `unshare` /
/// `setns` are not encoded here as strings — they're applied directly
/// in `apply_seccomp` via the `seccompiler` rule builder.
///
/// The unit test `seccomp_denylist_is_frozen` pins this list. Modifying
/// it requires updating that snapshot, which acts as a tripwire for
/// "future contributor silently shrinks the denylist" regressions.
pub const SECCOMP_DENYLIST_SIMPLE: &[&str] = &[
    "mount",
    "umount",
    "umount2",
    "pivot_root",
    "chroot",
    "kexec_load",
    "kexec_file_load",
    "init_module",
    "finit_module",
    "delete_module",
    "bpf",
    "perf_event_open",
    "ptrace",
    // `process_vm_readv` / `process_vm_writev` are a cross-process memory
    // read/write primitive independent of `ptrace` (no PTRACE_ATTACH, no
    // `/proc/<pid>/mem` open). Denying `ptrace` alone does NOT close them.
    // Critical in the `allow_fork=true` path, where bwrap does NOT
    // `--unshare-pid`, so the target shares the host PID namespace and
    // could read `aleph-server`'s address space (API keys, the proxy
    // bridge's in-flight traffic). Codex denies all three together; we
    // match that. EPERM (not SIGKILL) keeps benign callers survivable.
    "process_vm_readv",
    "process_vm_writev",
    "keyctl",
    "add_key",
    "request_key",
    "userfaultfd",
    "io_uring_setup",
    "io_uring_register",
    "io_uring_enter",
    "mknod",
    "mknodat",
    "swapon",
    "swapoff",
    "nfsservctl",
    "syslog",
    "reboot",
    "setns",
    // Filesystem-handle escape primitives. Aleph confines the filesystem with
    // Landlock, which is **path-based**: every rule is a `PathBeneath` over a
    // permitted directory hierarchy. `open_by_handle_at(2)` opens a file from
    // an opaque `file_handle` (obtained via `name_to_handle_at(2)`) WITHOUT a
    // pathname, so the kernel's path-walk — and therefore the Landlock
    // hierarchy check — is bypassed entirely: a handle to an inode outside the
    // permitted roots reopens it. Denying both closes that seam between the
    // Landlock (path) and seccomp (syscall) confinement layers. Neither is used
    // by ordinary tooling (`open_by_handle_at` needs `CAP_DAC_READ_SEARCH` and
    // appears only in NFS servers / backup daemons), so EPERM is non-disruptive.
    "open_by_handle_at",
    "name_to_handle_at",
];

/// Frozen socket-control denylist applied in [`SeccompNetworkMode::UnixOnly`]
/// (i.e. `NetworkPolicy::None`, network fully disabled). bwrap's
/// `--unshare-net` already strips every interface, and the socket-family gate
/// below already limits `socket(2)`/`socketpair(2)` to `AF_UNIX` and denies
/// `connect(2)`. These syscalls close the remaining socket *operation* surface:
/// a sandboxed process that retains or inherits a socket fd (e.g. an `AF_UNIX`
/// fd it is allowed to open) can otherwise still `bind`/`listen`/`accept` to
/// stand up an unexpected IPC endpoint, or `sendto`/`recvmmsg`/`setsockopt` on
/// an existing fd. Denying them unconditionally turns each into early `EPERM`
/// with a clear audit trail. Mirrors codex
/// `install_network_seccomp_filter_on_current_thread` (Restricted arm).
///
/// `recvfrom` is **deliberately excluded**: tools like `cargo clippy` rely on a
/// `socketpair` + child-process pattern that needs `recvfrom` on the local
/// `AF_UNIX` pair. Denying it would break otherwise-benign offline tooling,
/// exactly as codex documents. The pinning test `seccomp_socket_control_denylist_is_frozen`
/// guards this list against silent edits.
pub const SECCOMP_SOCKET_CONTROL_DENYLIST: &[&str] = &[
    "accept",
    "accept4",
    "bind",
    "listen",
    "getpeername",
    "getsockname",
    "shutdown",
    "sendto",
    "sendmmsg",
    "recvmmsg",
    "getsockopt",
    "setsockopt",
];

/// Build a `LinuxInitPolicy` from caller-side `SandboxCapabilities`
/// (cross-platform). The driver calls this on the host before invoking
/// `sandbox-init` so the policy travels as serialized JSON.
pub fn policy_from_capabilities(
    caps: &SandboxCapabilities,
    cwd: &std::path::Path,
    require_landlock: bool,
) -> LinuxInitPolicy {
    let mut read_paths: Vec<PathBuf> = SYSTEM_READ_PATHS.iter().map(PathBuf::from).collect();
    for p in &caps.fs_read {
        // rust-doctor-disable-next-line excessive-clone
        read_paths.push(p.clone());
    }
    // Workspace cwd is always writable — it's the whole point of the
    // workspace sandbox. Caller-supplied fs_write paths come after.
    let mut write_paths: Vec<PathBuf> = vec![cwd.to_path_buf()];
    for p in &caps.fs_write {
        // rust-doctor-disable-next-line excessive-clone
        write_paths.push(p.clone());
    }
    let seccomp_net = seccomp_net_for(&caps.network);
    LinuxInitPolicy {
        read_paths,
        write_paths,
        require_landlock,
        seccomp_net,
    }
}

/// Map a high-level [`NetworkPolicy`](crate::sandbox::capabilities::NetworkPolicy)
/// to the concrete seccomp socket gate:
///
/// - `None` → [`SeccompNetworkMode::UnixOnly`] (defense-in-depth over
///   `--unshare-net`).
/// - `AllowHosts` whose hosts are *all* loopback literals → the
///   proxy-collapsed form that `WorkspaceSandbox::maybe_spawn_proxy`
///   produces once the managed proxy + netns→UDS→loopback bridge are
///   live → [`SeccompNetworkMode::ProxyRouted`]. The detection mirrors
///   `BubblewrapDriver`'s own loopback dispatch, so the seccomp mode and
///   the bwrap `--unshare-net` arm always agree.
/// - `AllowAll`, or raw (non-loopback) `AllowHosts` →
///   [`SeccompNetworkMode::Unrestricted`] (seccomp cannot filter by IP).
fn seccomp_net_for(network: &crate::sandbox::capabilities::NetworkPolicy) -> SeccompNetworkMode {
    use crate::sandbox::capabilities::NetworkPolicy;
    match network {
        NetworkPolicy::None => SeccompNetworkMode::UnixOnly,
        NetworkPolicy::AllowAll => SeccompNetworkMode::Unrestricted,
        NetworkPolicy::AllowHosts { hosts }
            if !hosts.is_empty() && hosts.iter().all(|h| is_loopback(h)) =>
        {
            SeccompNetworkMode::ProxyRouted
        }
        NetworkPolicy::AllowHosts { .. } => SeccompNetworkMode::Unrestricted,
    }
}

/// Loopback-literal test, kept local to avoid coupling `sandbox_init` to the
/// Linux-only `bwrap` module (this fn is compiled cross-platform for unit
/// tests). Semantics match `bwrap::is_loopback_literal`.
fn is_loopback(host: &str) -> bool {
    host == "127.0.0.1"
        || host == "::1"
        || host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

// ---------------------------------------------------------------------------
// Phase B proxy-bridge env rewrite (cross-platform: pure string logic, unit-
// tested on dev boxes; the actual fork/bind lives in the Linux section).
// ---------------------------------------------------------------------------

/// Rewrite a proxy URL so its host becomes `127.0.0.1` and its port becomes
/// `local_port`, preserving scheme, path, and the bare-authority (no-scheme)
/// shape. The pre-rewrite value points at the host managed proxy; after the
/// netns-side local bridge binds a fresh loopback port, `sandbox-init` swaps
/// the port in. Modeled on codex `proxy_routing::rewrite_proxy_env_value`.
///
/// Returns `None` if the value cannot be parsed as a URL/authority.
///
/// `dead_code` allow: the only non-test caller is the Linux-gated
/// `activate_proxy_routes_from_env`, so on macOS/Windows `cargo check` (no
/// tests) sees it as unused.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn rewrite_proxy_env_value(proxy_url: &str, local_port: u16) -> Option<String> {
    let had_scheme = proxy_url.contains("://");
    let candidate = if had_scheme {
        proxy_url.to_string()
    } else {
        format!("http://{proxy_url}")
    };

    let mut parsed = url::Url::parse(&candidate).ok()?;
    parsed.set_host(Some("127.0.0.1")).ok()?;
    parsed.set_port(Some(local_port)).ok()?;
    let mut rewritten = parsed.to_string();
    if !had_scheme {
        rewritten = rewritten
            .strip_prefix("http://")
            .unwrap_or(rewritten.as_str())
            .to_string();
    }
    // `Url::to_string` re-adds a trailing slash for a bare authority; drop it
    // when the original had no path/query/fragment so we don't perturb the
    // value's shape (some clients are picky about a stray `/`).
    if !proxy_url.ends_with('/')
        && !proxy_url.contains('?')
        && !proxy_url.contains('#')
        && rewritten.ends_with('/')
    {
        rewritten.pop();
    }
    Some(rewritten)
}

// ---------------------------------------------------------------------------
// Linux-only entry point + LSM application.
// ---------------------------------------------------------------------------

/// Top-level entry point for the `sandbox-init` subcommand. Never
/// returns: either `execvp`s the target on success, or `process::exit`s
/// on policy / kernel failure.
///
/// Exit codes (per spec §6):
/// - 64 → landlock unavailable and `require_landlock=true`
/// - 65 → seccomp filter rejected by the kernel (unrecoverable)
/// - 66 → cannot parse argv (bad policy JSON, missing `--`, etc.)
/// - 67 → `execvp` failed (target binary not found or not executable)
/// - 68 → proxy-route bridge activation failed (Phase B netns bridge); fail
///   closed rather than run the target with broken egress control.
#[cfg(target_os = "linux")]
pub fn run_init(args: Vec<String>) -> ! {
    use std::os::unix::process::CommandExt as _;
    use std::process::Command;

    let parsed = match parse_init_args(&args) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("aleph sandbox-init: argv parse failed: {e}");
            std::process::exit(66);
        }
    };

    // PR_SET_NO_NEW_PRIVS is required before seccomp arms safely (else
    // setuid binaries we exec could regain privileges). Cheap, idempotent.
    if let Err(e) = set_no_new_privs() {
        eprintln!("aleph sandbox-init: prctl(PR_SET_NO_NEW_PRIVS) failed: {e}");
        std::process::exit(65);
    }

    // Phase B — activate the netns→UDS→loopback proxy bridge BEFORE landlock
    // and seccomp. The local bridge is `fork()`ed here (safe: `sandbox-init`
    // is single-threaded — dispatched in `main()` before any tokio runtime)
    // and must (a) outlive our `execvp` of the target and (b) keep filesystem
    // access to the UDS that landlock would otherwise strip. Forking it first
    // leaves the trusted bridge unrestricted while the target still inherits
    // the full landlock+seccomp lockdown. No-op when the env var is absent
    // (NetworkPolicy other than proxied AllowHosts).
    if let Err(e) = activate_proxy_routes_from_env() {
        eprintln!("aleph sandbox-init: proxy route bridge activation failed: {e}");
        std::process::exit(68);
    }

    if let Err(e) = apply_landlock(&parsed.policy) {
        if parsed.policy.require_landlock {
            eprintln!("aleph sandbox-init: landlock required but unavailable: {e}");
            std::process::exit(64);
        }
        eprintln!(
            "aleph sandbox-init: landlock unavailable on this kernel ({e}); \
             continuing with bwrap+seccomp only"
        );
    }

    if let Err(e) = apply_seccomp(&parsed.policy) {
        eprintln!("aleph sandbox-init: seccomp filter rejected: {e}");
        std::process::exit(65);
    }

    let mut cmd = Command::new(&parsed.target);
    cmd.args(&parsed.target_args);
    // `exec` replaces the process image; on success this never returns.
    let err = cmd.exec();
    eprintln!(
        "aleph sandbox-init: execvp({:?}) failed: {}",
        parsed.target, err
    );
    std::process::exit(67);
}

#[cfg(not(target_os = "linux"))]
pub fn run_init(_args: Vec<String>) -> ! {
    eprintln!("aleph sandbox-init: only supported on Linux");
    std::process::exit(78); // EX_CONFIG: unsupported on this platform
}

/// Output of `parse_init_args`. `target_args` is the slice after `--`.
/// `dead_code` allow: on macOS / Windows no caller exists at build time
/// (`run_init` is Linux-only), but unit tests reference it cross-platform.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Debug)]
struct ParsedInitArgs {
    policy: LinuxInitPolicy,
    target: String,
    target_args: Vec<String>,
}

/// argv layout: `[--policy <json> -- <target> <target-args...>]`.
/// (The leading `sandbox-init` subcommand name is stripped by the CLI
/// dispatcher before calling `run_init`.)
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_init_args(args: &[String]) -> Result<ParsedInitArgs, String> {
    let mut policy_json: Option<&str> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--policy" => {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| "--policy requires a value".to_string())?;
                policy_json = Some(v.as_str());
                i += 2;
            }
            "--" => {
                i += 1;
                break;
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    let policy_str = policy_json.ok_or_else(|| "missing --policy".to_string())?;
    let policy: LinuxInitPolicy =
        serde_json::from_str(policy_str).map_err(|e| format!("--policy JSON parse error: {e}"))?;

    let target = args
        .get(i)
        .ok_or_else(|| "missing target program after `--`".to_string())?
        // rust-doctor-disable-next-line excessive-clone
        .clone();
    let target_args = args[i + 1..].to_vec();

    Ok(ParsedInitArgs {
        policy,
        target,
        target_args,
    })
}

#[cfg(target_os = "linux")]
fn set_no_new_privs() -> Result<(), std::io::Error> {
    // SAFETY: prctl(PR_SET_NO_NEW_PRIVS, 1) is documented as always-safe.
    // rust-doctor-disable-next-line unsafe-block-audit
    let rc = unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1u64, 0u64, 0u64, 0u64) };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// Phase B activation, run inside the netns before landlock/seccomp.
///
/// Reads the [`crate::sandbox::proxy::PROXY_ROUTE_SPEC_ENV`] spec (a no-op if
/// absent), forks a local bridge that exposes the bind-mounted UDS as a fresh
/// loopback port, rewrites each listed proxy env var to that port, then strips
/// the spec var so the untrusted target never sees the routing internals.
#[cfg(target_os = "linux")]
fn activate_proxy_routes_from_env() -> std::io::Result<()> {
    use crate::sandbox::proxy::{ProxyRouteSpec, PROXY_ROUTE_SPEC_ENV};

    let Ok(raw) = std::env::var(PROXY_ROUTE_SPEC_ENV) else {
        return Ok(()); // not a proxied AllowHosts run
    };
    let spec = ProxyRouteSpec::from_env_json(&raw).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("parse proxy route spec: {e}"),
        )
    })?;

    let local_port = spawn_local_bridge(&spec.uds_path)?;

    for key in &spec.env_keys {
        // The pre-rewrite value points at the (unreachable-from-netns) host
        // proxy; swap its port to the local bridge. If a key is unset we
        // synthesize a plain http authority — the bridge speaks both HTTP
        // CONNECT and SOCKS5 by first-byte detection, so http:// is a safe
        // default for clients that only honor the env var.
        let current = std::env::var(key).unwrap_or_default();
        let rewritten = if current.is_empty() {
            format!("http://127.0.0.1:{local_port}")
        } else {
            rewrite_proxy_env_value(&current, local_port).ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("could not rewrite proxy env var {key}"),
                )
            })?
        };
        std::env::set_var(key, rewritten);
    }

    std::env::remove_var(PROXY_ROUTE_SPEC_ENV);
    Ok(())
}

/// Fork a child process that bridges a fresh loopback TCP port to `uds_path`
/// inside the netns, returning the bound port. The child survives our later
/// `execvp` (it is a separate process) and dies with us via `PR_SET_PDEATHSIG`.
/// Safe because `sandbox-init` is single-threaded at this point.
#[cfg(target_os = "linux")]
fn spawn_local_bridge(uds_path: &std::path::Path) -> std::io::Result<u16> {
    use std::fs::File;
    use std::io::Read as _;
    use std::os::fd::FromRawFd as _;

    let (read_fd, write_fd) = create_ready_pipe()?;
    // SAFETY: `fork()` is called in a single-threaded process before any async runtime.
    // rust-doctor-disable-next-line unsafe-block-audit
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        let err = std::io::Error::last_os_error();
        let _ = close_fd(read_fd);
        let _ = close_fd(write_fd);
        return Err(err);
    }

    if pid == 0 {
        // Child: close the read end, run the bridge forever. Any failure →
        // _exit(1) so the parent's read_exact sees EOF and surfaces an error.
        if close_fd(read_fd).is_err() {
            // SAFETY: `_exit` terminates the child without running destructors.
            // rust-doctor-disable-next-line unsafe-block-audit
            unsafe { libc::_exit(1) };
        }
        if run_local_bridge(uds_path, write_fd).is_err() {
            // SAFETY: `_exit` terminates the child without running destructors.
            // rust-doctor-disable-next-line unsafe-block-audit
            unsafe { libc::_exit(1) };
        }
        // SAFETY: `_exit` terminates the child after the bridge is running.
        // rust-doctor-disable-next-line unsafe-block-audit
        unsafe { libc::_exit(0) };
    }

    // Parent: read the bound port the child reports once it is listening.
    close_fd(write_fd)?;
    let mut port_bytes = [0u8; 2];
    // SAFETY: `read_fd` is a freshly-created, owned pipe read end.
    // rust-doctor-disable-next-line unsafe-block-audit
    let mut read_file = unsafe { File::from_raw_fd(read_fd) };
    read_file.read_exact(&mut port_bytes)?;
    Ok(u16::from_be_bytes(port_bytes))
}

#[cfg(target_os = "linux")]
fn run_local_bridge(uds_path: &std::path::Path, ready_fd: libc::c_int) -> std::io::Result<()> {
    use std::fs::File;
    use std::io::Write as _;
    use std::net::{Ipv4Addr, TcpListener};
    use std::os::fd::FromRawFd as _;
    use std::os::unix::net::UnixStream;

    set_parent_death_signal()?;

    // bwrap brings `lo` up as part of `--unshare-net` setup, so a plain
    // loopback bind succeeds without needing CAP_NET_ADMIN (which `--cap-drop
    // ALL` may have removed). This is why we omit codex's ioctl-based
    // `ensure_loopback_interface_up` fallback — bwrap owns that step.
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let port = listener.local_addr()?.port();

    // SAFETY: `ready_fd` is the owned write end of the readiness pipe.
    // rust-doctor-disable-next-line unsafe-block-audit
    let mut ready_file = unsafe { File::from_raw_fd(ready_fd) };
    ready_file.write_all(&port.to_be_bytes())?;
    drop(ready_file);

    let uds_path = uds_path.to_path_buf();
    loop {
        let (tcp_stream, _) = listener.accept()?;
        // rust-doctor-disable-next-line excessive-clone
        let socket_path = uds_path.clone();
        std::thread::spawn(move || {
            let unix_stream = match UnixStream::connect(socket_path) {
                Ok(stream) => stream,
                Err(_) => return,
            };
            let _ = proxy_bidirectional(tcp_stream, unix_stream);
        });
    }
}

/// Shovel bytes both ways between a loopback TCP stream and the UDS that
/// reaches the host bridge. One thread per direction; returns when either
/// side half-closes. Mirrors codex `proxy_routing::proxy_bidirectional`.
#[cfg(target_os = "linux")]
fn proxy_bidirectional(
    mut tcp_stream: std::net::TcpStream,
    mut unix_stream: std::os::unix::net::UnixStream,
) -> std::io::Result<()> {
    let mut tcp_reader = tcp_stream.try_clone()?;
    let mut unix_writer = unix_stream.try_clone()?;
    let tcp_to_unix = std::thread::spawn(move || std::io::copy(&mut tcp_reader, &mut unix_writer));
    let unix_to_tcp = std::io::copy(&mut unix_stream, &mut tcp_stream);
    let tcp_to_unix = tcp_to_unix
        .join()
        .map_err(|_| std::io::Error::other("bridge copy thread panicked"))?;
    tcp_to_unix?;
    unix_to_tcp?;
    Ok(())
}

/// Create a close-on-exec pipe used to hand the bound port (or a failure EOF)
/// from the forked bridge child back to the parent.
#[cfg(target_os = "linux")]
fn create_ready_pipe() -> std::io::Result<(libc::c_int, libc::c_int)> {
    let mut fds = [0; 2];
    // SAFETY: `pipe2` writes exactly two fds into the provided array.
    // rust-doctor-disable-next-line unsafe-block-audit
    let rc = unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok((fds[0], fds[1]))
}

#[cfg(target_os = "linux")]
fn close_fd(fd: libc::c_int) -> std::io::Result<()> {
    // SAFETY: closing an owned fd exactly once.
    // rust-doctor-disable-next-line unsafe-block-audit
    let rc = unsafe { libc::close(fd) };
    if rc < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// Ask the kernel to `SIGTERM` the bridge child when its parent dies, so a
/// crashed `sandbox-init` never leaks a bridge process. Mirrors codex.
#[cfg(target_os = "linux")]
fn set_parent_death_signal() -> std::io::Result<()> {
    // SAFETY: `getppid` returns the parent PID without side effects.
    // Cache before prctl to avoid TOCTOU: the parent could die after
    // prctl succeeds but before getppid returns, making the child appear
    // adopted by init (PID 1) and falsely reporting the parent as dead.
    // rust-doctor-disable-next-line unsafe-block-audit
    let parent_pid = unsafe { libc::getppid() };
    // SAFETY: prctl(PR_SET_PDEATHSIG) only sets a per-task attribute.
    // rust-doctor-disable-next-line unsafe-block-audit
    let rc = unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) };
    if rc != 0 {
        Err(std::io::Error::last_os_error())
    } else if parent_pid == 1 {
        Err(std::io::Error::other("parent process already exited"))
    } else {
        Ok(())
    }
}

#[cfg(target_os = "linux")]
// rust-doctor-disable-next-line high-cyclomatic-complexity
fn apply_landlock(policy: &LinuxInitPolicy) -> Result<(), String> {
    use landlock::{
        Access, AccessFs, CompatLevel, Compatible, PathBeneath, PathFd, RestrictionStatus, Ruleset,
        RulesetAttr, RulesetCreatedAttr, RulesetStatus, ABI,
    };

    // Negotiate against the highest Landlock ABI this build knows about
    // (V5: ioctl-on-device control, on top of V2 cross-directory
    // refer/rename, V3 truncate, V4 TCP port rules) and let the kernel
    // degrade gracefully via `CompatLevel::BestEffort`. The previous code
    // pinned `ABI::V1`, which permanently capped enforcement at the V1
    // access set even on modern kernels — leaving `LANDLOCK_ACCESS_FS_REFER`
    // (a cross-mount rename/link escape vector), `TRUNCATE` and `IOCTL_DEV`
    // unrestricted. BestEffort preserves the old behaviour on V1-only
    // kernels (unsupported access bits are silently dropped, no error)
    // while tightening enforcement wherever the running kernel supports it.
    // Mirrors codex `linux-sandbox/src/landlock.rs`.
    let abi = ABI::V5;
    let mut ruleset = Ruleset::default()
        .set_compatibility(CompatLevel::BestEffort)
        .handle_access(AccessFs::from_all(abi))
        .map_err(|e| format!("Ruleset::handle_access: {e}"))?
        .create()
        .map_err(|e| format!("Ruleset::create (kernel may lack landlock): {e}"))?;

    let read_mask = AccessFs::from_read(abi) | AccessFs::Execute;
    let write_mask = AccessFs::from_all(abi);

    // System-minimum + caller-supplied readable hierarchies.
    for path in &policy.read_paths {
        if let Ok(fd) = PathFd::new(path) {
            ruleset = ruleset
                .add_rule(PathBeneath::new(fd, read_mask))
                .map_err(|e| format!("add read rule {path:?}: {e}"))?;
        }
    }

    // Read-only pseudo-devices (`/dev/urandom`, `/dev/random`). Absent on
    // some minimal devtmpfs layouts → skip silently.
    for path in SYSTEM_DEVICE_RO_PATHS {
        if let Ok(fd) = PathFd::new(path) {
            ruleset = ruleset
                .add_rule(PathBeneath::new(fd, read_mask))
                .map_err(|e| format!("add device read rule {path}: {e}"))?;
        }
    }

    // Workspace cwd + caller-supplied writable hierarchies.
    for path in &policy.write_paths {
        if let Ok(fd) = PathFd::new(path) {
            ruleset = ruleset
                .add_rule(PathBeneath::new(fd, write_mask))
                .map_err(|e| format!("add write rule {path:?}: {e}"))?;
        }
    }

    // Read+write pseudo-devices (`/dev/null`, `/dev/zero`, `/dev/full`,
    // `/dev/tty`). Without these the default-deny ruleset blocks shell
    // redirection and tty writes inside the sandbox.
    for path in SYSTEM_DEVICE_RW_PATHS {
        if let Ok(fd) = PathFd::new(path) {
            ruleset = ruleset
                .add_rule(PathBeneath::new(fd, write_mask))
                .map_err(|e| format!("add device write rule {path}: {e}"))?;
        }
    }

    let status: RestrictionStatus = ruleset
        .restrict_self()
        .map_err(|e| format!("restrict_self: {e}"))?;
    match status.ruleset {
        RulesetStatus::NotEnforced => {
            return Err("landlock kernel-side enforcement returned NotEnforced".into());
        }
        RulesetStatus::PartiallyEnforced => {
            // BestEffort downgraded some access bits because the running
            // kernel exposes a lower ABI than V5. The filesystem is still
            // confined at the kernel's supported level — surface it for
            // audit, not as a failure.
            tracing::debug!(
                "landlock partially enforced (kernel ABI < V5); \
                 confinement active at the highest supported level"
            );
        }
        RulesetStatus::FullyEnforced => {
            tracing::debug!("landlock fully enforced at ABI V5");
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn apply_seccomp(policy: &LinuxInitPolicy) -> Result<(), String> {
    use std::collections::BTreeMap;

    use seccompiler::{
        BpfProgram, SeccompAction, SeccompCmpArgLen, SeccompCmpOp, SeccompCondition, SeccompFilter,
        SeccompRule, TargetArch,
    };

    let target_arch: TargetArch = if cfg!(target_arch = "x86_64") {
        TargetArch::x86_64
    } else if cfg!(target_arch = "aarch64") {
        TargetArch::aarch64
    } else {
        return Err(format!(
            "unsupported target_arch for seccomp filter: {}",
            std::env::consts::ARCH
        ));
    };

    let mut rules: BTreeMap<i64, Vec<SeccompRule>> = BTreeMap::new();
    let mut skipped: Vec<&str> = Vec::new();
    for name in SECCOMP_DENYLIST_SIMPLE {
        // `syscall_nr` returns `None` for denylist entries that do not exist
        // on the current architecture (e.g. `umount`, `mknod`, `nfsservctl`
        // are absent on arm64 / modern kernels). The denylist is an
        // intentional superset, so a missing syscall is a no-op — there is
        // nothing to deny — not a fatal error. Skip it rather than aborting
        // the whole filter (which would `exit(65)` and kill every sandboxed
        // process on those arches).
        let Some(nr) = syscall_nr(name) else {
            skipped.push(name);
            continue;
        };
        // Empty rule vec = unconditional match for this syscall.
        rules.insert(nr, vec![]);
    }
    // Surface the architecture-skipped entries so an operator on arm64 /
    // a future-arch kernel can audit the actual protection level (not
    // just trust the x86_64-shaped denylist).
    if !skipped.is_empty() {
        tracing::warn!(
            target: "sandbox",
            arch = std::env::consts::ARCH,
            skipped = ?skipped,
            "[sandbox] seccomp denylist entries skipped on this architecture; \
             the catastrophic floor is partial here"
        );
    }

    // clone/unshare with CLONE_NEWUSER → deny (nested user-namespace
    // escape). CLONE_NEWUSER bit = 0x10000000.
    const CLONE_NEWUSER: u64 = 0x1000_0000;
    let nuser_cond = SeccompCondition::new(
        0,
        SeccompCmpArgLen::Qword,
        SeccompCmpOp::MaskedEq(CLONE_NEWUSER),
        CLONE_NEWUSER,
    )
    .map_err(|e| format!("build CLONE_NEWUSER condition: {e}"))?;
    if let Some(nr) = syscall_nr("clone") {
        rules.insert(
            nr,
            vec![SeccompRule::new(vec![nuser_cond.clone()])
                .map_err(|e| format!("build clone rule: {e}"))?],
        );
    }
    if let Some(nr) = syscall_nr("unshare") {
        rules.insert(
            nr,
            vec![SeccompRule::new(vec![nuser_cond])
                .map_err(|e| format!("build unshare rule: {e}"))?],
        );
    }

    // Socket-family gate, codex-aligned. The bwrap `--unshare-net` netns
    // already strips interfaces, but a process inside it can still call
    // `socket(AF_INET, ...)` / `connect()` and fail at runtime with cryptic
    // errors — these rules turn that into early EPERM with a clear audit
    // trail, and in `ProxyRouted` mode narrow the surface to exactly what
    // the loopback bridge needs. Sock-domain args are 32-bit in the syscall
    // ABI, so use `Dword` even though the AF_* constants are small.
    apply_socket_gate(&mut rules, policy.seccomp_net)?;

    let filter = SeccompFilter::new(
        rules,
        // Default: allow everything not on the denylist.
        SeccompAction::Allow,
        // Matched syscalls return EPERM.
        SeccompAction::Errno(libc::EPERM as u32),
        target_arch,
    )
    .map_err(|e| format!("SeccompFilter::new: {e}"))?;

    let prog: BpfProgram = filter
        .try_into()
        .map_err(|e| format!("compile BPF program: {e}"))?;

    seccompiler::apply_filter(&prog).map_err(|e| format!("apply_filter: {e}"))?;
    Ok(())
}

/// Lower a [`SeccompNetworkMode`] into concrete `socket`/`socketpair`/`connect`
/// argument rules, inserting them into `rules`. Factored out of
/// [`apply_seccomp`] so the three-way policy is one exhaustive `match`
/// instead of an `if` chain. Mirrors codex
/// `install_network_seccomp_filter_on_current_thread`.
#[cfg(target_os = "linux")]
fn apply_socket_gate(
    rules: &mut std::collections::BTreeMap<i64, Vec<seccompiler::SeccompRule>>,
    mode: SeccompNetworkMode,
) -> Result<(), String> {
    use seccompiler::{SeccompCmpArgLen, SeccompCmpOp, SeccompCondition, SeccompRule};

    match mode {
        // No socket-family filtering — kernel namespace + driver rules carry
        // network policy (AllowAll / raw AllowHosts).
        SeccompNetworkMode::Unrestricted => {}
        SeccompNetworkMode::UnixOnly => {
            // Allow only AF_UNIX; a single `Ne AF_UNIX` condition denies
            // AF_INET/AF_INET6/AF_NETLINK/AF_PACKET/AF_VSOCK/… automatically.
            let non_unix = SeccompRule::new(vec![SeccompCondition::new(
                0, // socket/socketpair arg0 = domain
                SeccompCmpArgLen::Dword,
                SeccompCmpOp::Ne,
                libc::AF_UNIX as u64,
            )
            .map_err(|e| format!("build AF_UNIX != condition: {e}"))?])
            .map_err(|e| format!("build non-AF_UNIX socket rule: {e}"))?;
            if let Some(nr) = syscall_nr("socket") {
                rules.insert(nr, vec![non_unix.clone()]);
            }
            if let Some(nr) = syscall_nr("socketpair") {
                rules.insert(nr, vec![non_unix]);
            }
            // Network disabled → deny connect outright. AF_UNIX connect() is
            // caught too, but sandboxed targets rarely depend on it and EPERM
            // propagates as a clear error rather than a crash.
            if let Some(nr) = syscall_nr("connect") {
                rules.insert(nr, vec![]);
            }
            // Close the remaining socket-operation surface (bind/listen/accept/
            // sendto/…). Each missing-on-this-arch entry resolves to `None` and
            // is skipped, matching the `SECCOMP_DENYLIST_SIMPLE` loop. See
            // `SECCOMP_SOCKET_CONTROL_DENYLIST` for the rationale and the
            // deliberate `recvfrom` exclusion.
            for name in SECCOMP_SOCKET_CONTROL_DENYLIST {
                if let Some(nr) = syscall_nr(name) {
                    rules.insert(nr, vec![]);
                }
            }
        }
        SeccompNetworkMode::ProxyRouted => {
            // Allow only AF_INET / AF_INET6 (to reach the loopback TCP
            // bridge); deny everything else. Two `Ne` conditions ANDed in one
            // rule → matches (denies) when the domain is neither IP family.
            let non_ip = SeccompRule::new(vec![
                SeccompCondition::new(
                    0,
                    SeccompCmpArgLen::Dword,
                    SeccompCmpOp::Ne,
                    libc::AF_INET as u64,
                )
                .map_err(|e| format!("build AF_INET != condition: {e}"))?,
                SeccompCondition::new(
                    0,
                    SeccompCmpArgLen::Dword,
                    SeccompCmpOp::Ne,
                    libc::AF_INET6 as u64,
                )
                .map_err(|e| format!("build AF_INET6 != condition: {e}"))?,
            ])
            .map_err(|e| format!("build non-IP socket rule: {e}"))?;
            // Deny AF_UNIX socketpairs so the target can't open a fresh Unix
            // socket to sidestep the routed bridge.
            let unix_pair = SeccompRule::new(vec![SeccompCondition::new(
                0,
                SeccompCmpArgLen::Dword,
                SeccompCmpOp::Eq,
                libc::AF_UNIX as u64,
            )
            .map_err(|e| format!("build AF_UNIX == condition: {e}"))?])
            .map_err(|e| format!("build AF_UNIX socketpair rule: {e}"))?;
            if let Some(nr) = syscall_nr("socket") {
                rules.insert(nr, vec![non_ip]);
            }
            if let Some(nr) = syscall_nr("socketpair") {
                rules.insert(nr, vec![unix_pair]);
            }
            // `connect` stays allowed — the target must reach 127.0.0.1:<bridge>.
        }
    }
    Ok(())
}

/// Map syscall name → arch-specific syscall number. Returns `None` for
/// names that don't exist on the current architecture (e.g.
/// `nfsservctl` was removed in 5.17).
#[cfg(target_os = "linux")]
// rust-doctor-disable-next-line high-cyclomatic-complexity
fn syscall_nr(name: &str) -> Option<i64> {
    // Translates via libc::SYS_* constants. Only the names actually used
    // by `SECCOMP_DENYLIST_SIMPLE` need to be listed.
    let nr: i64 = match name {
        "mount" => libc::SYS_mount,
        "umount2" => libc::SYS_umount2,
        // umount (no '2') was removed from glibc on most arches; the
        // 2.x form is what userland calls.
        "umount" => return None,
        "pivot_root" => libc::SYS_pivot_root,
        "chroot" => libc::SYS_chroot,
        "kexec_load" => libc::SYS_kexec_load,
        "kexec_file_load" => libc::SYS_kexec_file_load,
        "init_module" => libc::SYS_init_module,
        "finit_module" => libc::SYS_finit_module,
        "delete_module" => libc::SYS_delete_module,
        "bpf" => libc::SYS_bpf,
        "perf_event_open" => libc::SYS_perf_event_open,
        "ptrace" => libc::SYS_ptrace,
        "process_vm_readv" => libc::SYS_process_vm_readv,
        "process_vm_writev" => libc::SYS_process_vm_writev,
        "keyctl" => libc::SYS_keyctl,
        "add_key" => libc::SYS_add_key,
        "request_key" => libc::SYS_request_key,
        "userfaultfd" => libc::SYS_userfaultfd,
        "io_uring_setup" => libc::SYS_io_uring_setup,
        "io_uring_register" => libc::SYS_io_uring_register,
        "io_uring_enter" => libc::SYS_io_uring_enter,
        "mknod" => return None, // arm64 removed mknod in favor of mknodat
        "mknodat" => libc::SYS_mknodat,
        "swapon" => libc::SYS_swapon,
        "swapoff" => libc::SYS_swapoff,
        "nfsservctl" => return None, // removed in 5.17
        "syslog" => libc::SYS_syslog,
        "reboot" => libc::SYS_reboot,
        "setns" => libc::SYS_setns,
        // Filesystem-handle escape primitives (bypass path-based Landlock).
        "open_by_handle_at" => libc::SYS_open_by_handle_at,
        "name_to_handle_at" => libc::SYS_name_to_handle_at,
        "clone" => libc::SYS_clone,
        "unshare" => libc::SYS_unshare,
        "socket" => libc::SYS_socket,
        "socketpair" => libc::SYS_socketpair,
        "connect" => libc::SYS_connect,
        // Socket-control surface denied in UnixOnly mode (network disabled).
        // All present on x86_64/aarch64 — the only arches `apply_seccomp`
        // accepts — so none of these `return None` in practice, but the
        // fallthrough below keeps the contract (unknown name → skip) intact.
        "accept" => libc::SYS_accept,
        "accept4" => libc::SYS_accept4,
        "bind" => libc::SYS_bind,
        "listen" => libc::SYS_listen,
        "getpeername" => libc::SYS_getpeername,
        "getsockname" => libc::SYS_getsockname,
        "shutdown" => libc::SYS_shutdown,
        "sendto" => libc::SYS_sendto,
        "sendmmsg" => libc::SYS_sendmmsg,
        "recvmmsg" => libc::SYS_recvmmsg,
        "getsockopt" => libc::SYS_getsockopt,
        "setsockopt" => libc::SYS_setsockopt,
        _ => return None,
    };
    Some(nr)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_includes_system_read_paths() {
        let caps = SandboxCapabilities::strict();
        let cwd = std::path::Path::new("/workspace");
        let p = policy_from_capabilities(&caps, cwd, false);
        for sys in SYSTEM_READ_PATHS {
            assert!(
                p.read_paths.iter().any(|x| x == std::path::Path::new(sys)),
                "missing system read path {sys}"
            );
        }
    }

    #[test]
    fn policy_includes_cwd_as_writable() {
        let caps = SandboxCapabilities::strict();
        let cwd = std::path::Path::new("/workspace/session-abc");
        let p = policy_from_capabilities(&caps, cwd, false);
        assert!(p.write_paths.iter().any(|x| x == cwd));
    }

    #[test]
    fn policy_threads_fs_read_and_fs_write_from_caps() {
        let caps = SandboxCapabilities {
            fs_read: vec![PathBuf::from("/home/user/data")],
            fs_write: vec![PathBuf::from("/tmp/scratch")],
            ..SandboxCapabilities::strict()
        };
        let p = policy_from_capabilities(&caps, std::path::Path::new("/workspace"), false);
        assert!(p
            .read_paths
            .iter()
            .any(|x| x == std::path::Path::new("/home/user/data")));
        assert!(p
            .write_paths
            .iter()
            .any(|x| x == std::path::Path::new("/tmp/scratch")));
    }

    #[test]
    fn policy_threads_require_landlock_flag() {
        let caps = SandboxCapabilities::strict();
        let cwd = std::path::Path::new("/workspace");
        assert!(!policy_from_capabilities(&caps, cwd, false).require_landlock);
        assert!(policy_from_capabilities(&caps, cwd, true).require_landlock);
    }

    #[test]
    fn policy_round_trips_through_json() {
        let original = LinuxInitPolicy {
            read_paths: vec!["/usr".into(), "/lib".into()],
            write_paths: vec!["/workspace".into()],
            require_landlock: true,
            seccomp_net: SeccompNetworkMode::ProxyRouted,
        };
        let json = serde_json::to_string(&original).unwrap();
        let parsed: LinuxInitPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(original, parsed);
    }

    #[test]
    fn seccomp_net_defaults_unrestricted_for_forward_compat() {
        // On-the-wire JSON that predates `seccomp_net` (and the older
        // `deny_network_sockets` bool) must still deserialize. The absent
        // field defaults to `Unrestricted`, matching the legacy
        // `deny_network_sockets:false` no-filtering behavior so an old
        // policy never silently gains a restriction it didn't intend.
        let json = r#"{"read_paths":[],"write_paths":[]}"#;
        let parsed: LinuxInitPolicy = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.seccomp_net, SeccompNetworkMode::Unrestricted);
        // The legacy bool field is now ignored (serde drops unknown keys),
        // so a stale `deny_network_sockets` does not break parsing.
        let legacy = r#"{"read_paths":[],"write_paths":[],"deny_network_sockets":true}"#;
        let parsed: LinuxInitPolicy = serde_json::from_str(legacy).unwrap();
        assert_eq!(parsed.seccomp_net, SeccompNetworkMode::Unrestricted);
    }

    #[test]
    fn seccomp_net_serializes_snake_case() {
        // Wire shape is `snake_case` so the variant names read naturally in
        // an audited policy blob.
        let json = serde_json::to_string(&SeccompNetworkMode::UnixOnly).unwrap();
        assert_eq!(json, r#""unix_only""#);
        let json = serde_json::to_string(&SeccompNetworkMode::ProxyRouted).unwrap();
        assert_eq!(json, r#""proxy_routed""#);
    }

    #[test]
    fn policy_threads_seccomp_net_from_capabilities() {
        use crate::sandbox::capabilities::NetworkPolicy;
        let cwd = std::path::Path::new("/workspace");
        // None → UnixOnly (defense in depth on top of bwrap --unshare-net).
        let none_caps = SandboxCapabilities {
            network: NetworkPolicy::None,
            ..SandboxCapabilities::strict()
        };
        assert_eq!(
            policy_from_capabilities(&none_caps, cwd, false).seccomp_net,
            SeccompNetworkMode::UnixOnly
        );

        // AllowAll → Unrestricted (kernel ns + driver rules carry policy).
        let all_caps = SandboxCapabilities {
            network: NetworkPolicy::AllowAll,
            ..SandboxCapabilities::strict()
        };
        assert_eq!(
            policy_from_capabilities(&all_caps, cwd, false).seccomp_net,
            SeccompNetworkMode::Unrestricted
        );

        // Raw (non-loopback) AllowHosts → Unrestricted: seccomp can't filter
        // by IP, and the bwrap driver hard-errors this form anyway.
        let raw_hosts = SandboxCapabilities {
            network: NetworkPolicy::AllowHosts {
                hosts: vec!["10.0.0.1".into()],
            },
            ..SandboxCapabilities::strict()
        };
        assert_eq!(
            policy_from_capabilities(&raw_hosts, cwd, false).seccomp_net,
            SeccompNetworkMode::Unrestricted
        );

        // Proxy-collapsed loopback AllowHosts → ProxyRouted (the new
        // least-privilege mode for egress behind the netns→UDS→loopback
        // bridge). Detection matches the driver's own loopback dispatch.
        for loopback in [
            vec!["127.0.0.1".to_string()],
            vec!["::1".to_string()],
            vec!["localhost".to_string()],
        ] {
            let caps = SandboxCapabilities {
                network: NetworkPolicy::AllowHosts { hosts: loopback },
                ..SandboxCapabilities::strict()
            };
            assert_eq!(
                policy_from_capabilities(&caps, cwd, false).seccomp_net,
                SeccompNetworkMode::ProxyRouted
            );
        }

        // A mixed allowlist (one non-loopback host) is NOT proxy-routed —
        // it stays Unrestricted so we never under-filter a real IP behind a
        // loopback sibling.
        let mixed = SandboxCapabilities {
            network: NetworkPolicy::AllowHosts {
                hosts: vec!["127.0.0.1".into(), "10.0.0.1".into()],
            },
            ..SandboxCapabilities::strict()
        };
        assert_eq!(
            policy_from_capabilities(&mixed, cwd, false).seccomp_net,
            SeccompNetworkMode::Unrestricted
        );
    }

    /// Pins the denylist by hash so any addition / removal / reorder is
    /// caught by CI. Snapshot owner: spec SP-2 § 5. If you need to
    /// change the list, also update the spec and bump this hash.
    #[test]
    fn seccomp_denylist_is_frozen() {
        let joined = SECCOMP_DENYLIST_SIMPLE.join(",");
        let expected = "mount,umount,umount2,pivot_root,chroot,kexec_load,kexec_file_load,\
                        init_module,finit_module,delete_module,bpf,perf_event_open,ptrace,\
                        process_vm_readv,process_vm_writev,keyctl,add_key,request_key,\
                        userfaultfd,io_uring_setup,io_uring_register,io_uring_enter,mknod,\
                        mknodat,swapon,swapoff,nfsservctl,syslog,reboot,setns,\
                        open_by_handle_at,name_to_handle_at";
        assert_eq!(
            joined, expected,
            "seccomp denylist changed — update SP-2 spec"
        );
        // The FS-handle escape primitives must stay denied: they reopen inodes
        // from opaque handles, bypassing path-based Landlock confinement.
        assert!(SECCOMP_DENYLIST_SIMPLE.contains(&"open_by_handle_at"));
        assert!(SECCOMP_DENYLIST_SIMPLE.contains(&"name_to_handle_at"));
    }

    /// Pins the pseudo-device grant sets. These compensate for the
    /// `from_all`-handled (default-deny) landlock ruleset over bwrap's
    /// `--dev /dev` devtmpfs; shrinking them silently re-breaks
    /// `/dev/null` redirection or `/dev/urandom` reads inside the sandbox.
    #[test]
    fn device_path_grants_are_frozen() {
        assert_eq!(
            SYSTEM_DEVICE_RW_PATHS,
            &["/dev/null", "/dev/zero", "/dev/full", "/dev/tty"],
        );
        assert_eq!(SYSTEM_DEVICE_RO_PATHS, &["/dev/random", "/dev/urandom"]);
        // No path may appear in both sets (a RW grant would shadow a RO one
        // and muddy the intent).
        for rw in SYSTEM_DEVICE_RW_PATHS {
            assert!(
                !SYSTEM_DEVICE_RO_PATHS.contains(rw),
                "{rw} is in both RW and RO device sets"
            );
        }
        // Every device grant lives under /dev — guards against a typo
        // accidentally granting a broad hierarchy.
        for p in SYSTEM_DEVICE_RW_PATHS.iter().chain(SYSTEM_DEVICE_RO_PATHS) {
            assert!(p.starts_with("/dev/"), "{p} is not under /dev/");
        }
    }

    #[test]
    fn parse_init_args_extracts_policy_and_target() {
        let policy = LinuxInitPolicy {
            read_paths: vec!["/usr".into()],
            write_paths: vec!["/workspace".into()],
            require_landlock: false,
            seccomp_net: SeccompNetworkMode::Unrestricted,
        };
        let json = serde_json::to_string(&policy).unwrap();
        let argv = vec![
            "--policy".to_string(),
            json,
            "--".to_string(),
            "/usr/bin/python".to_string(),
            "-c".to_string(),
            "print('hi')".to_string(),
        ];
        let parsed = parse_init_args(&argv).unwrap();
        assert_eq!(parsed.policy, policy);
        assert_eq!(parsed.target, "/usr/bin/python");
        assert_eq!(parsed.target_args, vec!["-c", "print('hi')"]);
    }

    #[test]
    fn parse_init_args_rejects_missing_policy() {
        let argv = vec!["--".to_string(), "/usr/bin/true".to_string()];
        let err = parse_init_args(&argv).unwrap_err();
        assert!(err.contains("missing --policy"), "got: {err}");
    }

    #[test]
    fn parse_init_args_rejects_missing_target() {
        let argv = vec![
            "--policy".to_string(),
            "{\"read_paths\":[],\"write_paths\":[]}".to_string(),
            "--".to_string(),
        ];
        let err = parse_init_args(&argv).unwrap_err();
        assert!(err.contains("missing target"), "got: {err}");
    }

    #[test]
    fn parse_init_args_rejects_bad_json() {
        let argv = vec![
            "--policy".to_string(),
            "not json".to_string(),
            "--".to_string(),
            "/usr/bin/true".to_string(),
        ];
        let err = parse_init_args(&argv).unwrap_err();
        assert!(err.contains("JSON parse error"), "got: {err}");
    }

    #[test]
    fn rewrite_proxy_env_value_swaps_host_and_port_keeping_scheme() {
        // socks5h scheme + explicit loopback host → port swapped, scheme kept.
        assert_eq!(
            rewrite_proxy_env_value("socks5h://127.0.0.1:8081", 43210).as_deref(),
            Some("socks5h://127.0.0.1:43210")
        );
        // http scheme.
        assert_eq!(
            rewrite_proxy_env_value("http://127.0.0.1:3128", 50000).as_deref(),
            Some("http://127.0.0.1:50000")
        );
    }

    #[test]
    fn rewrite_proxy_env_value_preserves_bare_authority_shape() {
        // No scheme, no path → stays bare (no scheme re-added, no trailing /).
        assert_eq!(
            rewrite_proxy_env_value("127.0.0.1:3128", 40000).as_deref(),
            Some("127.0.0.1:40000")
        );
    }

    #[test]
    fn rewrite_proxy_env_value_rejects_garbage() {
        assert_eq!(rewrite_proxy_env_value("", 40000), None);
        assert_eq!(rewrite_proxy_env_value("http://", 40000), None);
    }

    /// Pins the UnixOnly socket-control denylist (codex Restricted parity).
    /// Shrinking it silently re-opens the socket-operation surface in the
    /// network-disabled tier; growing it without intent risks breaking benign
    /// offline tooling. Acts as a tripwire alongside `seccomp_denylist_is_frozen`.
    #[test]
    fn seccomp_socket_control_denylist_is_frozen() {
        let joined = SECCOMP_SOCKET_CONTROL_DENYLIST.join(",");
        let expected = "accept,accept4,bind,listen,getpeername,getsockname,shutdown,\
                        sendto,sendmmsg,recvmmsg,getsockopt,setsockopt";
        assert_eq!(
            joined, expected,
            "UnixOnly socket-control denylist changed — confirm codex parity intent"
        );
        // `recvfrom` is deliberately allowed (cargo clippy socketpair pattern).
        assert!(
            !SECCOMP_SOCKET_CONTROL_DENYLIST.contains(&"recvfrom"),
            "recvfrom must stay allowed — denying it breaks socketpair-based tooling"
        );
        // The control surface is disjoint from the always-on danger denylist;
        // duplicates would mean a control syscall is denied even in AllowAll.
        for name in SECCOMP_SOCKET_CONTROL_DENYLIST {
            assert!(
                !SECCOMP_DENYLIST_SIMPLE.contains(name),
                "{name} appears in both the always-on and UnixOnly-only denylists"
            );
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn socket_gate_unix_only_denies_full_control_surface() {
        use std::collections::BTreeMap;

        // UnixOnly: every socket-control syscall is an unconditional deny.
        let mut rules: BTreeMap<i64, Vec<seccompiler::SeccompRule>> = BTreeMap::new();
        apply_socket_gate(&mut rules, SeccompNetworkMode::UnixOnly).unwrap();
        for name in SECCOMP_SOCKET_CONTROL_DENYLIST {
            let nr = syscall_nr(name).expect("control syscall must resolve on x86_64/aarch64");
            let rule = rules
                .get(&nr)
                .unwrap_or_else(|| panic!("{name} must be denied in UnixOnly mode"));
            assert!(
                rule.is_empty(),
                "{name} must be an unconditional (empty-condition) deny"
            );
        }
        // connect is denied; socket/socketpair carry the AF_UNIX conditional rule.
        assert!(rules.contains_key(&libc::SYS_connect));

        // ProxyRouted and Unrestricted must NOT pull in the control surface —
        // those tiers legitimately reach the loopback bridge / open networking.
        for mode in [
            SeccompNetworkMode::ProxyRouted,
            SeccompNetworkMode::Unrestricted,
        ] {
            let mut other: BTreeMap<i64, Vec<seccompiler::SeccompRule>> = BTreeMap::new();
            apply_socket_gate(&mut other, mode).unwrap();
            for name in SECCOMP_SOCKET_CONTROL_DENYLIST {
                let nr = syscall_nr(name).unwrap();
                assert!(
                    !other.contains_key(&nr),
                    "{name} must not be denied in {mode:?} mode"
                );
            }
        }
    }
}
